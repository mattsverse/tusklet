use crate::model::{ContainerState, Database};
use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use std::{
    collections::HashMap,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::Duration,
};
use wait_timeout::ChildExt;

const TIMEOUT: Duration = Duration::from_secs(20);
const TRANSFER_TIMEOUT: Duration = Duration::from_mins(30);
const LABEL: &str = "dev.tusklet.managed=true";

#[derive(Clone)]
pub struct Docker {
    executable: PathBuf,
}

impl Default for Docker {
    fn default() -> Self {
        // Finder-launched applications may not inherit a shell's PATH.
        let executable = if cfg!(target_os = "macos") {
            [
                "/usr/local/bin/docker",
                "/opt/homebrew/bin/docker",
                "/Applications/Docker.app/Contents/Resources/bin/docker",
            ]
            .into_iter()
            .map(PathBuf::from)
            .find(|path| path.is_file())
            .unwrap_or_else(|| "docker".into())
        } else {
            "docker".into()
        };
        Self { executable }
    }
}

impl Docker {
    #[must_use]
    pub fn with_executable(executable: PathBuf) -> Self {
        Self { executable }
    }

    fn command(&self, args: &[String]) -> Command {
        let mut command = Command::new(&self.executable);
        command.args(args).stdin(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        command
    }

    fn run(&self, args: &[&str]) -> Result<String> {
        let args = args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>();
        execute(self.command(&args), TIMEOUT, false)
    }

    /// List app-owned containers, including stopped containers.
    ///
    /// # Errors
    /// Returns an error if Docker is unavailable or its response cannot be parsed.
    pub fn states(&self) -> Result<HashMap<String, ContainerState>> {
        let output = self.run(&[
            "ps",
            "--all",
            "--filter",
            &format!("label={LABEL}"),
            "--format",
            "{{json .}}",
        ])?;
        parse_states(&output)
    }

    /// List locally cached official PostgreSQL tags without contacting a registry.
    ///
    /// # Errors
    /// Returns an error if Docker cannot list local images.
    pub fn local_images(&self) -> Result<Vec<String>> {
        let output = self.run(&["image", "ls", "--format", "{{.Repository}}:{{.Tag}}"])?;
        let mut tags: Vec<_> = output
            .lines()
            .filter_map(|line| line.strip_prefix("postgres:"))
            .filter(|tag| *tag != "<none>")
            .map(str::to_owned)
            .collect();
        tags.sort();
        tags.dedup();
        Ok(tags)
    }

    fn pull(&self, db: &Database) -> Result<()> {
        execute(
            self.command(&["pull".into(), db.image()]),
            TRANSFER_TIMEOUT,
            false,
        )?;
        Ok(())
    }

    /// Create or start a database, optionally downloading a missing image.
    ///
    /// # Errors
    /// Fails on invalid configuration, a missing offline image, an ownership
    /// mismatch, or a Docker failure. Existing data volumes are retained.
    pub fn start(&self, db: &Database, allow_pull: bool) -> Result<()> {
        let state = self.states()?.get(&db.container_name()).cloned();
        if state.as_ref().is_some_and(ContainerState::is_active) {
            return Ok(());
        }
        if state.is_none() {
            if !self.local_images()?.contains(&db.config.tag) {
                ensure!(
                    allow_pull,
                    "Image {} is not available locally. Turn off offline mode to download it.",
                    db.image()
                );
                self.pull(db)?;
            }
            let mut command = self.command(&create_args(db)?);
            // Only the variable's name appears in the process arguments.
            command.env("POSTGRES_PASSWORD", &db.password);
            execute(command, TIMEOUT, false).map_err(|error| {
                anyhow::anyhow!(error.to_string().replace(&db.password, "[redacted]"))
            })?;
        }
        self.verify_owner(db)?;
        self.run(&["start", &db.container_name()])?;
        Ok(())
    }

    fn verify_owner(&self, db: &Database) -> Result<()> {
        let owner = self.run(&[
            "inspect",
            "--format",
            "{{ index .Config.Labels \"dev.tusklet.database\" }}",
            &db.container_name(),
        ])?;
        ensure!(
            owner.trim() == db.id,
            "Refusing to change a container not owned by this Tusklet database."
        );
        Ok(())
    }

    /// Stop the owned container without removing its data.
    ///
    /// # Errors
    /// Fails if ownership cannot be verified or Docker cannot stop the container.
    pub fn stop(&self, db: &Database) -> Result<()> {
        self.verify_owner(db)?;
        self.run(&["stop", "--time", "10", &db.container_name()])?;
        Ok(())
    }

    /// Remove a stopped, owned container, preserving its named data volume.
    ///
    /// # Errors
    /// Fails if the container is active, ownership differs, or Docker fails.
    pub fn remove_container(&self, db: &Database) -> Result<()> {
        if let Some(state) = self.states()?.get(&db.container_name()) {
            ensure!(!state.is_active(), "Stop the database first.");
            self.verify_owner(db)?;
            // Remove only anonymous image-declared volumes; Docker retains the
            // explicitly named PostgreSQL data volume with --volumes.
            self.run(&["rm", "--volumes", &db.container_name()])?;
        }
        Ok(())
    }

    /// Fetch the latest bounded log tail from a container.
    ///
    /// # Errors
    /// Fails if Docker cannot read the container logs.
    pub fn logs(&self, db: &Database) -> Result<String> {
        // A bounded tail is re-read while Follow is enabled. This survives restarts
        // and avoids orphaned `docker logs --follow` processes on selection changes.
        self.run(&[
            "logs",
            "--tail",
            "400",
            "--timestamps",
            &db.container_name(),
        ])
    }

    /// Export a custom-format backup without overwriting the destination.
    ///
    /// # Errors
    /// Fails on an ownership mismatch, filesystem error, or failed `pg_dump`.
    /// Failed exports leave no partial backup at the destination.
    pub fn dump(&self, db: &Database, destination: &Path) -> Result<()> {
        self.verify_owner(db)?;
        ensure!(
            !destination.exists(),
            "That file already exists. Choose a new filename."
        );
        let parent = destination
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let temporary =
            tempfile::NamedTempFile::new_in(parent).context("Cannot write to that folder.")?;
        let args = strings(&[
            "exec",
            &db.container_name(),
            "pg_dump",
            "--format=custom",
            "--no-owner",
            "--no-privileges",
            "-U",
            &db.config.user,
            "--dbname",
            &db.config.database,
        ]);
        let mut command = self.command(&args);
        command.stdout(Stdio::from(temporary.reopen()?));
        execute(command, TRANSFER_TIMEOUT, true)?;
        temporary.as_file().sync_all()?;
        temporary
            .persist_noclobber(destination)
            .context("Could not save the dump. The destination may already exist.")?;
        Ok(())
    }

    /// Restore a trusted custom-format or plain SQL backup in a transaction.
    ///
    /// # Errors
    /// Fails if the source cannot be read, ownership differs, or PostgreSQL
    /// rejects the restore. Transactional SQL is rolled back on failure.
    pub fn restore(&self, db: &Database, source: &Path) -> Result<()> {
        self.verify_owner(db)?;
        let mut file = File::open(source).context("Cannot open the backup file.")?;
        let mut magic = [0u8; 5];
        let read = file.read(&mut magic)?;
        ensure!(read > 0, "The backup file is empty.");
        let mut args = strings(&["exec", "--interactive", &db.container_name()]);
        if &magic == b"PGDMP" {
            args.extend(strings(&[
                "pg_restore",
                "--single-transaction",
                "--exit-on-error",
                "--no-owner",
                "--no-privileges",
                "-U",
                &db.config.user,
                "--dbname",
                &db.config.database,
            ]));
        } else {
            args.extend(strings(&[
                "psql",
                "-X",
                "--single-transaction",
                "--set",
                "ON_ERROR_STOP=1",
                "-U",
                &db.config.user,
                "--dbname",
                &db.config.database,
            ]));
        }
        let mut command = self.command(&args);
        command.stdin(Stdio::from(File::open(source)?));
        execute(command, TRANSFER_TIMEOUT, false)?;
        Ok(())
    }
}

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| (*s).into()).collect()
}

fn create_args(db: &Database) -> Result<Vec<String>> {
    db.config.validate()?;
    let mut args = strings(&[
        "create",
        "--name",
        &db.container_name(),
        "--label",
        LABEL,
        "--label",
        &format!("dev.tusklet.database={}", db.id),
        "--publish",
        &format!("127.0.0.1:{}:5432", db.port),
        "--mount",
        &format!(
            "type=volume,source={},target=/var/lib/postgresql",
            db.volume_name()
        ),
        // Explicit PGDATA avoids the version-dependent defaults (changed in PG18).
        // Keep it outside /var/lib/postgresql/data, which older images declare as VOLUME.
        "--env",
        "PGDATA=/var/lib/postgresql/tusklet",
        "--env",
        "POSTGRES_PASSWORD",
        "--env",
        &format!("POSTGRES_USER={}", db.config.user),
        "--env",
        &format!("POSTGRES_DB={}", db.config.database),
        "--health-cmd",
        &format!("pg_isready -U {} -d {}", db.config.user, db.config.database),
        "--health-interval",
        "3s",
        "--health-timeout",
        "3s",
        "--health-retries",
        "20",
        "--log-opt",
        "max-size=10m",
        "--log-opt",
        "max-file=3",
        "--pull=never",
        &db.image(),
        "postgres",
    ]);
    args.extend(db.config.postgres_args()?);
    Ok(args)
}

fn parse_states(output: &str) -> Result<HashMap<String, ContainerState>> {
    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct Row {
        names: String,
        state: String,
        status: String,
    }
    output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let row: Row =
                serde_json::from_str(line).context("Unexpected response from Docker.")?;
            let state = match row.state.as_str() {
                "running" if row.status.contains("unhealthy") => ContainerState::Unhealthy,
                "running" if row.status.contains("health: starting") => ContainerState::Starting,
                "running" => ContainerState::Running,
                "restarting" => ContainerState::Starting,
                "created" | "exited" | "dead" => ContainerState::Stopped,
                _ => ContainerState::Unknown,
            };
            Ok((row.names, state))
        })
        .collect()
}

/// Drain both pipes concurrently to avoid deadlocks, and cap retained diagnostic
/// output. Dump/restore data travels directly through files instead of memory.
fn drain(mut reader: impl Read) -> std::io::Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut buffer = [0; 8192];
    loop {
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        output.extend_from_slice(&buffer[..n]);
        if output.len() > 2 * 1024 * 1024 {
            output.drain(..output.len() - 2 * 1024 * 1024);
        }
    }
    Ok(output)
}

fn execute(mut command: Command, timeout: Duration, stdout_is_file: bool) -> Result<String> {
    if !stdout_is_file {
        command.stdout(Stdio::piped());
    }
    command.stderr(Stdio::piped());
    let mut child = command.spawn().context("Could not launch Docker. Install Docker Desktop or Docker Engine and make sure the docker CLI is available.")?;
    let stdout = child
        .stdout
        .take()
        .map(|pipe| thread::spawn(move || drain(pipe)));
    let stderr = child
        .stderr
        .take()
        .map(|pipe| thread::spawn(move || drain(pipe)));
    let Some(status) = child.wait_timeout(timeout)? else {
        let _ = child.kill();
        let _ = child.wait();
        bail!(
            "Docker timed out. Check the daemon and retry. A timed-out container operation may still have completed; refresh its state."
        );
    };
    let read = |handle: Option<thread::JoinHandle<std::io::Result<Vec<u8>>>>| -> Result<String> {
        let bytes = match handle {
            Some(handle) => handle
                .join()
                .map_err(|_| anyhow::anyhow!("Could not read Docker output."))??,
            None => vec![],
        };
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    };
    let out = read(stdout)?;
    let err = read(stderr)?;
    ensure!(
        status.success(),
        "Docker operation failed: {}",
        if err.trim().is_empty() {
            out.trim()
        } else {
            err.trim()
        }
    );
    let mut result = out;
    if !err.is_empty() {
        result.push_str(&err);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::DatabaseConfig;
    #[test]
    fn container_uses_localhost_persistent_volume_and_never_exposes_password_in_args() {
        let db = Database {
            id: "test-id".into(),
            project_id: "p".into(),
            config: DatabaseConfig {
                name: "Test".into(),
                command: "-c wal_level=logical".into(),
                ..Default::default()
            },
            password: "test-only-secret".into(),
            port: 5437,
        };
        let args = create_args(&db).unwrap();
        assert!(args.contains(&"127.0.0.1:5437:5432".into()));
        assert!(args.contains(&"PGDATA=/var/lib/postgresql/tusklet".into()));
        assert!(args.contains(&"--pull=never".into()));
        assert!(args.iter().all(|arg| !arg.contains(&db.password)));
        assert!(args.ends_with(&strings(&[
            "postgres:18",
            "postgres",
            "-c",
            "wal_level=logical"
        ])));
    }
    #[test]
    fn distinguishes_health_and_external_stops() {
        let states = parse_states("{\"Names\":\"a\",\"State\":\"running\",\"Status\":\"Up (health: starting)\"}\n{\"Names\":\"b\",\"State\":\"exited\",\"Status\":\"Exited\"}\n{\"Names\":\"c\",\"State\":\"running\",\"Status\":\"Up (unhealthy)\"}").unwrap();
        assert_eq!(states["a"], ContainerState::Starting);
        assert_eq!(states["b"], ContainerState::Stopped);
        assert_eq!(states["c"], ContainerState::Unhealthy);
        assert!(parse_states("garbage").is_err());
    }
}
