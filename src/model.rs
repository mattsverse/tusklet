use anyhow::{Result, bail, ensure};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DatabaseConfig {
    pub name: String,
    pub tag: String,
    pub database: String,
    pub user: String,
    pub reserved_port: Option<u16>,
    pub command: String,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            tag: "18".into(),
            database: "postgres".into(),
            user: "postgres".into(),
            reserved_port: None,
            command: String::new(),
        }
    }
}

/// Validate a project or database display name.
///
/// # Errors
/// Rejects blank names, names over 80 characters, and control characters.
pub fn validate_name(value: &str) -> Result<()> {
    ensure!(!value.trim().is_empty(), "A name is required.");
    ensure!(
        value.chars().count() <= 80,
        "Names must be at most 80 characters."
    );
    ensure!(
        !value.chars().any(char::is_control),
        "Names cannot contain control characters."
    );
    Ok(())
}

impl DatabaseConfig {
    /// Validate configuration before persisting or creating a container.
    ///
    /// # Errors
    /// Rejects invalid names, image tags, identifiers, ports, or server arguments.
    pub fn validate(&self) -> Result<()> {
        validate_name(&self.name)?;
        let tag = self.tag.as_bytes();
        ensure!(
            !tag.is_empty() && tag.len() <= 128,
            "Enter a PostgreSQL image tag, such as 18 or 17-alpine."
        );
        ensure!(
            tag[0].is_ascii_alphanumeric() || tag[0] == b'_',
            "Invalid image tag."
        );
        ensure!(
            tag.iter()
                .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(c)),
            "Use an official postgres image tag, not a repository or URL."
        );
        for (label, value) in [("Database", &self.database), ("User", &self.user)] {
            ensure!(
                !value.is_empty() && value.len() <= 63,
                "{label} must be 1–63 bytes."
            );
            ensure!(
                value
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'_')
                    && !value.as_bytes()[0].is_ascii_digit(),
                "{label} must start with a letter or underscore and contain only letters, numbers, and underscores."
            );
        }
        if let Some(port) = self.reserved_port {
            ensure!(port >= 1024, "Use a reserved port between 1024 and 65535.");
        }
        self.postgres_args()?;
        Ok(())
    }

    /// Parse argument quoting without invoking a shell. Network and storage settings
    /// belong to `HandyPOS` so the container remains reachable and data stays durable.
    ///
    /// # Errors
    /// Rejects malformed quoting, non-setting arguments, and changes to managed
    /// network or storage settings.
    pub fn postgres_args(&self) -> Result<Vec<String>> {
        ensure!(
            self.command.len() <= 8192,
            "PostgreSQL arguments are too long."
        );
        let mut args = shell_words::split(&self.command)
            .map_err(|_| anyhow::anyhow!("Unclosed quote in PostgreSQL arguments."))?;
        if args.first().is_some_and(|arg| arg == "postgres") {
            args.remove(0);
        }
        let mut index = 0;
        while index < args.len() {
            let arg = &args[index];
            let setting = if arg == "-c" {
                index += 1;
                args.get(index)
                    .ok_or_else(|| anyhow::anyhow!("Each -c needs a setting=value argument."))?
                    .as_str()
            } else if let Some(value) = arg.strip_prefix("-c") {
                value
            } else if let Some(value) = arg.strip_prefix("--") {
                value
            } else {
                bail!(
                    "Use PostgreSQL settings in the form -c name=value (for example -c wal_level=logical)."
                );
            };
            let (key, value) = setting
                .split_once('=')
                .ok_or_else(|| anyhow::anyhow!("Each PostgreSQL setting needs name=value."))?;
            ensure!(
                !key.is_empty() && !value.is_empty(),
                "Each PostgreSQL setting needs name=value."
            );
            let normalized_key = key.trim().replace('-', "_").to_ascii_lowercase();
            ensure!(
                ![
                    "port",
                    "listen_addresses",
                    "data_directory",
                    "config_file",
                    "hba_file",
                    "ident_file"
                ]
                .contains(&normalized_key.as_str()),
                "{key} is managed by HandyPOS."
            );
            index += 1;
        }
        Ok(args)
    }
}

// Deliberately no Debug/Serialize: avoid accidentally exposing the password.
#[derive(Clone)]
pub struct Database {
    pub id: String,
    pub project_id: String,
    pub config: DatabaseConfig,
    pub password: String,
    pub port: u16,
}

impl Database {
    #[must_use]
    pub fn container_name(&self) -> String {
        format!("handypos-{}", self.id)
    }
    #[must_use]
    pub fn volume_name(&self) -> String {
        format!("handypos-{}-data", self.id)
    }
    #[must_use]
    pub fn image(&self) -> String {
        format!("postgres:{}", self.config.tag)
    }
    #[must_use]
    pub fn connection_url(&self) -> String {
        let encode = |s: &str| utf8_percent_encode(s, NON_ALPHANUMERIC).to_string();
        format!(
            "postgresql://{}:{}@127.0.0.1:{}/{}",
            encode(&self.config.user),
            encode(&self.password),
            self.port,
            encode(&self.config.database)
        )
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ContainerState {
    #[default]
    NotCreated,
    Running,
    Stopped,
    Starting,
    Unhealthy,
    Unknown,
}

impl ContainerState {
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::NotCreated => "Not started",
            Self::Running => "Running",
            Self::Stopped => "Stopped",
            Self::Starting => "Starting",
            Self::Unhealthy => "Unhealthy",
            Self::Unknown => "Unavailable",
        }
    }
    #[must_use]
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Running | Self::Starting | Self::Unhealthy)
    }
}

#[derive(Clone)]
pub struct DatabaseView {
    pub database: Database,
    pub state: ContainerState,
}

#[derive(Clone, Default)]
pub struct Snapshot {
    pub projects: Vec<Project>,
    pub databases: Vec<DatabaseView>,
    pub images: Vec<String>,
    pub docker_error: Option<String>,
    pub logs: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_tags_ports_and_names() {
        let mut config = DatabaseConfig {
            name: "Main".into(),
            ..Default::default()
        };
        for tag in ["18", "17.5-alpine", "latest", "19beta3"] {
            config.tag = tag.into();
            config.validate().unwrap();
        }
        for tag in ["", "postgres:18", "--privileged", "18;touch /tmp/test"] {
            config.tag = tag.into();
            assert!(config.validate().is_err());
        }
        config.tag = "18".into();
        config.reserved_port = Some(0);
        assert!(config.validate().is_err());
        config.reserved_port = Some(5432);
        config.database = "-bad".into();
        assert!(config.validate().is_err());
    }
    #[test]
    fn parses_quotes_without_shell_execution_and_protects_storage() {
        let mut config = DatabaseConfig {
            command: "postgres -c wal_level=logical -c \"application_name=hello world\"".into(),
            ..Default::default()
        };
        assert_eq!(
            config.postgres_args().unwrap(),
            [
                "-c",
                "wal_level=logical",
                "-c",
                "application_name=hello world"
            ]
        );
        for command in [
            "-c",
            "-c 'oops",
            "-D /tmp",
            "--port=1234",
            "-cdata_directory=/tmp",
            "-c listen_addresses=localhost",
            "--data-directory=/tmp",
            "-c 'PORT =5433'",
            "sh -c whoami",
        ] {
            config.command = command.into();
            assert!(config.postgres_args().is_err(), "{command}");
        }
    }
}
