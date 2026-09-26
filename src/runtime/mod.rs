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

mod target;
pub use target::{Engine, RuntimeSettings, Target};

const TIMEOUT: Duration = Duration::from_secs(20);
const TRANSFER_TIMEOUT: Duration = Duration::from_mins(30);
const LABEL: &str = "dev.tusklet.managed=true";

#[derive(Clone)]
pub struct ContainerRuntime {
    executable: PathBuf,
    engine: Engine,
    pub(crate) target: Option<Target>,
}

impl Default for ContainerRuntime {
    fn default() -> Self {
        Self::new(Engine::Docker)
    }
}

impl ContainerRuntime {
    #[must_use]
    pub fn new(engine: Engine) -> Self {
        Self::with_executable(engine, engine.executable())
    }

    #[must_use]
    pub fn with_executable(engine: Engine, executable: PathBuf) -> Self {
        Self {
            executable,
            engine,
            target: None,
        }
    }

    #[must_use]
    pub fn engine(&self) -> Engine {
        self.engine
    }

    #[must_use]
    pub fn with_target(mut self, target: Target) -> Self {
        self.target = Some(target);
        self
    }

    fn execute(
        &self,
        command: Command,
        timeout: Duration,
        stdout_is_file: bool,
    ) -> Result<CommandOutput> {
        execute(command, timeout, stdout_is_file, self.engine)
    }

    fn command(&self, args: &[String]) -> Command {
        let mut command = Command::new(&self.executable);
        if let Some(target) = &self.target {
            target.configure(&mut command);
        }
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
        Ok(self.execute(self.command(&args), TIMEOUT, false)?.stdout)
    }

    /// List app-owned containers, including stopped containers.
    ///
    /// # Errors
    /// Returns an error if the runtime is unavailable or its response cannot be parsed.
    pub fn states(&self) -> Result<HashMap<String, ContainerState>> {
        if self.engine == Engine::Podman {
            return self.podman_states();
        }
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

    fn podman_states(&self) -> Result<HashMap<String, ContainerState>> {
        let ids = self.run(&[
            "ps",
            "--all",
            "--filter",
            &format!("label={LABEL}"),
            "--format",
            "{{.ID}}",
        ])?;
        let mut states = HashMap::new();
        // Read only names and states; inspection must not collect environment secrets.
        // Batches keep command lines bounded when the workspace has many containers.
        let ids: Vec<_> = ids.split_whitespace().collect();
        for batch in ids.chunks(100) {
            let mut args = vec![
                "inspect",
                "--type",
                "container",
                "--format",
                r#"{"Name":{{json .Name}},"State":{{json .State}}}"#,
            ];
            args.extend_from_slice(batch);
            states.extend(parse_podman_states(&self.run(&args)?)?);
        }
        Ok(states)
    }

    /// List locally cached official PostgreSQL tags without contacting a registry.
    ///
    /// # Errors
    /// Returns an error if the runtime cannot list local images.
    pub fn local_images(&self) -> Result<Vec<String>> {
        let output = self.run(&["image", "ls", "--format", "{{.Repository}}:{{.Tag}}"])?;
        let mut tags: Vec<_> = output
            .lines()
            .filter_map(|line| {
                [
                    "postgres:",
                    "docker.io/library/postgres:",
                    "docker.io/postgres:",
                ]
                .iter()
                .find_map(|prefix| line.strip_prefix(prefix))
            })
            .filter(|tag| *tag != "<none>")
            .map(str::to_owned)
            .collect();
        tags.sort();
        tags.dedup();
        Ok(tags)
    }

    fn pull(&self, db: &Database) -> Result<()> {
        self.execute(
            self.command(&["pull".into(), db.image()]),
            TRANSFER_TIMEOUT,
            false,
        )?;
        Ok(())
    }

    /// Create or start a database using a cached image, downloading only if missing.
    ///
    /// # Errors
    /// Fails on invalid configuration, an unavailable image, an ownership
    /// mismatch, or a runtime failure. Existing data volumes are retained.
    pub fn start(&self, db: &Database) -> Result<()> {
        let state = self.states()?.get(&db.container_name()).cloned();
        if state.as_ref().is_some_and(ContainerState::is_active) {
            return Ok(());
        }
        ensure!(
            state != Some(ContainerState::Unknown),
            "Container state is unavailable. Refresh before starting it."
        );
        if state.is_none() {
            if !self.local_images()?.contains(&db.config.tag) {
                self.pull(db).with_context(|| {
                    format!(
                        "Image {} is not available locally and could not be downloaded. Check your connection to Docker Hub and try again.",
                        db.image()
                    )
                })?;
            }
            let mut command = self.command(&create_args(db, self.engine)?);
            // Only the variable's name appears in the process arguments.
            command.env("POSTGRES_PASSWORD", &db.password);
            self.execute(command, TIMEOUT, false).map_err(|error| {
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
    /// Fails if ownership cannot be verified or the runtime cannot stop the container.
    pub fn stop(&self, db: &Database) -> Result<()> {
        self.verify_owner(db)?;
        self.run(&["stop", "--time", "10", &db.container_name()])?;
        Ok(())
    }

    /// Remove a stopped, owned container, preserving its named data volume.
    ///
    /// # Errors
    /// Fails if the container is active, ownership differs, or the runtime fails.
    pub fn remove_container(&self, db: &Database) -> Result<()> {
        if let Some(state) = self.states()?.get(&db.container_name()) {
            ensure!(
                matches!(state, ContainerState::Stopped),
                "Stop the database first and wait for its state to be available."
            );
            self.verify_owner(db)?;
            // Remove only anonymous image-declared volumes; both engines retain the
            // explicitly named PostgreSQL data volume with --volumes.
            self.run(&["rm", "--volumes", &db.container_name()])?;
        }
        Ok(())
    }

    /// Permanently remove this database's named data volume, if it exists.
    /// Call after removing the stopped container. The runtime refuses volumes still in use.
    ///
    /// # Errors
    /// Fails if the runtime cannot list volumes or remove the database's volume.
    pub fn remove_data_volume(&self, db: &Database) -> Result<()> {
        let volume = db.volume_name();
        let names = self.run(&[
            "volume",
            "ls",
            "--format",
            "{{.Name}}",
            "--filter",
            &format!("name={volume}"),
        ])?;
        // Match exactly: A runtime's name filter can also return similarly named volumes.
        // An unstarted database (or a retry) may have no volume at all.
        if names.lines().any(|name| name == volume) {
            self.run(&["volume", "rm", &volume])?;
        }
        Ok(())
    }

    /// Fetch the latest bounded log tail from a container.
    ///
    /// # Errors
    /// Fails if the runtime cannot read the container logs.
    pub fn logs(&self, db: &Database) -> Result<String> {
        // A bounded tail is re-read while Follow is enabled. This survives restarts
        // and avoids orphaned `docker logs --follow` processes on selection changes.
        let args = strings(&[
            "logs",
            "--tail",
            "400",
            "--timestamps",
            &db.container_name(),
        ]);
        let output = self.execute(self.command(&args), TIMEOUT, false)?;
        // PostgreSQL log lines can arrive on either stream.
        Ok(output.stdout + &output.stderr)
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
        self.execute(command, TRANSFER_TIMEOUT, true)?;
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
        self.execute(command, TRANSFER_TIMEOUT, false)?;
        Ok(())
    }
}

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| (*s).into()).collect()
}

fn create_args(db: &Database, engine: Engine) -> Result<Vec<String>> {
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
    ]);
    args.extend(match engine {
        Engine::Docker => strings(&[
            "--log-driver",
            "json-file",
            "--log-opt",
            "max-size=10m",
            "--log-opt",
            "max-file=3",
        ]),
        Engine::Podman => strings(&["--log-driver", "k8s-file", "--log-opt", "max-size=10mb"]),
    });
    args.extend(strings(&["--pull=never", &db.image(), "postgres"]));
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

fn parse_podman_states(output: &str) -> Result<HashMap<String, ContainerState>> {
    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct Health {
        status: String,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct State {
        status: String,
        #[serde(default)]
        health: Option<Health>,
        #[serde(default)]
        healthcheck: Option<Health>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct Row {
        name: String,
        state: State,
    }
    output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let row: Row =
                serde_json::from_str(line).context("Unexpected response from Podman.")?;
            let health = row.state.health.or(row.state.healthcheck);
            let health = health.as_ref().map(|health| health.status.as_str());
            let state = match row.state.status.as_str() {
                "running" => match health {
                    Some("healthy") => ContainerState::Running,
                    Some("unhealthy") => ContainerState::Unhealthy,
                    // Managed containers have a health check. Never declare them ready
                    // before Podman has actually run it (including rootless setups).
                    _ => ContainerState::Starting,
                },
                "restarting" | "stopping" => ContainerState::Starting,
                "created" | "configured" | "initialized" | "stopped" | "exited" => {
                    ContainerState::Stopped
                }
                _ => ContainerState::Unknown,
            };
            Ok((row.name.trim_start_matches('/').to_owned(), state))
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

struct CommandOutput {
    stdout: String,
    stderr: String,
}

fn execute(
    mut command: Command,
    timeout: Duration,
    stdout_is_file: bool,
    engine: Engine,
) -> Result<CommandOutput> {
    if !stdout_is_file {
        command.stdout(Stdio::piped());
    }
    command.stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .with_context(|| format!("Could not launch {}. {}", engine.name(), engine.help()))?;
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
            "{} timed out. {} A timed-out operation may still have completed; refresh its state.",
            engine.name(),
            engine.help()
        );
    };
    let read = |handle: Option<thread::JoinHandle<std::io::Result<Vec<u8>>>>| -> Result<String> {
        let bytes = match handle {
            Some(handle) => handle
                .join()
                .map_err(|_| anyhow::anyhow!("Could not read container runtime output."))??,
            None => vec![],
        };
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    };
    let out = read(stdout)?;
    let err = read(stderr)?;
    ensure!(
        status.success(),
        "{} operation failed: {}",
        engine.name(),
        if err.trim().is_empty() {
            out.trim()
        } else {
            err.trim()
        }
    );
    Ok(CommandOutput {
        stdout: out,
        stderr: err,
    })
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
        let args = create_args(&db, Engine::Docker).unwrap();
        assert!(args.contains(&"127.0.0.1:5437:5432".into()));
        assert!(args.contains(&"PGDATA=/var/lib/postgresql/tusklet".into()));
        assert!(args.contains(&"--pull=never".into()));
        assert!(args.iter().all(|arg| !arg.contains(&db.password)));
        assert!(args.ends_with(&strings(&[
            "docker.io/library/postgres:18",
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
