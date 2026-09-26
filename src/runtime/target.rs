use super::{ContainerRuntime, strings};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, process::Command};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Engine {
    #[default]
    Docker,
    Podman,
}

impl Engine {
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Docker => "Docker",
            Self::Podman => "Podman",
        }
    }

    pub(super) fn executable(self) -> PathBuf {
        let name = match self {
            Self::Docker => "docker",
            Self::Podman => "podman",
        };
        if cfg!(target_os = "macos") {
            // Finder does not inherit the interactive shell's PATH.
            for candidate in [
                format!("/opt/homebrew/bin/{name}"),
                format!("/usr/local/bin/{name}"),
                format!("/opt/podman/bin/{name}"),
                format!("/Applications/Docker.app/Contents/Resources/bin/{name}"),
            ] {
                let path = PathBuf::from(candidate);
                if path.is_file() {
                    return path;
                }
            }
        }
        name.into()
    }

    #[must_use]
    pub fn help(self) -> &'static str {
        match self {
            Self::Docker => "Install the Docker CLI and start Docker Desktop or Docker Engine.",
            Self::Podman if cfg!(target_os = "linux") => {
                "Install Podman and check that it works for your current user."
            }
            Self::Podman => {
                "Install Podman, then run podman machine init once and podman machine start."
            }
        }
    }
}

/// A workspace owns one target. Defaults are consulted only until it is pinned.
#[derive(
    Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, sea_orm::FromJsonQueryResult,
)]
pub struct RuntimeSettings {
    pub engine: Engine,
    pub target: Option<Target>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Target {
    Docker {
        context: String,
        host: String,
    },
    PodmanLocal {
        graph_root: String,
    },
    PodmanConnection {
        name: String,
        uri: String,
        identity: String,
    },
}

impl Target {
    #[must_use]
    pub fn description(&self) -> String {
        match self {
            Self::Docker { context, host } => {
                if context.is_empty() {
                    host.clone()
                } else {
                    context.clone()
                }
            }
            Self::PodmanLocal { .. } => "Local Podman".into(),
            Self::PodmanConnection { name, .. } => name.clone(),
        }
    }

    pub(super) fn configure(&self, command: &mut Command) {
        match self {
            Self::Docker { host, .. } => {
                command.args(["--host", host]);
                command
                    .env_remove("DOCKER_HOST")
                    .env_remove("DOCKER_CONTEXT")
                    .env_remove("DOCKER_TLS_VERIFY")
                    .env_remove("DOCKER_TLS");
            }
            Self::PodmanLocal { .. } => {
                command.arg("--remote=false");
                command
                    .env_remove("CONTAINER_HOST")
                    .env_remove("CONTAINER_CONNECTION");
            }
            Self::PodmanConnection { uri, identity, .. } => {
                command.args(["--url", uri]);
                if !identity.is_empty() {
                    command.args(["--identity", identity]);
                }
                command
                    .env_remove("CONTAINER_HOST")
                    .env_remove("CONTAINER_CONNECTION")
                    .env_remove("CONTAINER_SSHKEY");
            }
        }
    }
}

impl ContainerRuntime {
    /// Resolve the user's current local target without changing containers.
    ///
    /// # Errors
    /// Fails on unavailable CLIs, invalid configuration, or remote targets.
    pub fn resolve_target(&self) -> Result<Target> {
        match self.engine {
            Engine::Docker => self.resolve_docker(),
            Engine::Podman => self.resolve_podman(),
        }
    }

    fn resolve_docker(&self) -> Result<Target> {
        let explicit_context = std::env::var("DOCKER_CONTEXT")
            .ok()
            .filter(|v| !v.is_empty());
        let explicit_host = std::env::var("DOCKER_HOST").ok().filter(|v| !v.is_empty());
        let (context, host) = if let (None, Some(host)) = (explicit_context, explicit_host) {
            (String::new(), host)
        } else {
            let context = self.run(&["context", "show"])?.trim().to_owned();
            let host = self.run(&[
                "context",
                "inspect",
                &context,
                "--format",
                "{{.Endpoints.docker.Host}}",
            ])?;
            (context, host.trim().to_owned())
        };
        ensure!(
            host.starts_with("unix:///") || host.starts_with("npipe://"),
            "Choose a local Docker context using a Unix socket or Windows named pipe. Remote Docker hosts are not supported."
        );
        Ok(Target::Docker { context, host })
    }

    fn resolve_podman(&self) -> Result<Target> {
        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct Connection {
            name: String,
            #[serde(rename = "URI")]
            uri: String,
            #[serde(default)]
            identity: String,
            #[serde(default)]
            default: bool,
            #[serde(default)]
            is_machine: bool,
        }
        let connection = std::env::var("CONTAINER_CONNECTION")
            .ok()
            .filter(|v| !v.is_empty());
        if connection.is_none()
            && let Ok(uri) = std::env::var("CONTAINER_HOST")
            && !uri.is_empty()
        {
            ensure!(
                uri.starts_with("unix:///"),
                "Select a local Podman machine connection instead of CONTAINER_HOST. Remote hosts are not supported."
            );
            return Ok(Target::PodmanConnection {
                name: "Local socket".into(),
                uri,
                identity: String::new(),
            });
        }
        if cfg!(target_os = "linux") && connection.is_none() {
            return Ok(Target::PodmanLocal {
                graph_root: self.local_graph_root()?,
            });
        }
        let connections: Vec<Connection> = serde_json::from_str(&self.run(&[
            "system",
            "connection",
            "list",
            "--format",
            "json",
        ])?)?;
        let selected = connections
            .into_iter()
            .find(|c| {
                connection
                    .as_ref()
                    .map_or(c.default, |name| c.name == *name)
            })
            .context(
                "No Podman connection found. Run podman machine init, then podman machine start.",
            )?;
        ensure!(
            selected.uri.starts_with("unix:///")
                || (selected.is_machine && local_machine_uri(&selected.uri)),
            "Only local Podman sockets and local Podman machines are supported."
        );
        Ok(Target::PodmanConnection {
            name: selected.name,
            uri: selected.uri,
            identity: selected.identity,
        })
    }

    fn local_graph_root(&self) -> Result<String> {
        // Explicit local mode avoids defaults or environment redirecting native Podman.
        let output = self.run(&["--remote=false", "info", "--format", "{{.Store.GraphRoot}}"])?;
        let root = output.trim();
        ensure!(
            !root.is_empty(),
            "Podman returned an empty storage location."
        );
        Ok(root.into())
    }

    pub(crate) fn verify_target(&self) -> Result<()> {
        if let Some(Target::PodmanLocal { graph_root }) = &self.target {
            ensure!(
                *graph_root == self.local_graph_root()?,
                "Podman's storage location changed. Restore the original Podman configuration to access this workspace's data."
            );
        }
        Ok(())
    }

    /// Build a command on this runtime's pinned target, also used by integration checks.
    #[must_use]
    pub fn cli_command(&self, args: &[&str]) -> Command {
        self.command(&strings(args))
    }
}

fn local_machine_uri(uri: &str) -> bool {
    let Some(authority) = uri.strip_prefix("ssh://").and_then(|s| s.split('/').next()) else {
        return false;
    };
    let host = authority.rsplit('@').next().unwrap_or("");
    ["127.0.0.1:", "localhost:", "[::1]:"]
        .iter()
        .any(|prefix| host.starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_commands_override_environment_target_selection() {
        for (engine, target, variables) in [
            (
                Engine::Docker,
                Target::Docker {
                    context: "saved".into(),
                    host: "unix:///saved/docker.sock".into(),
                },
                vec!["DOCKER_HOST", "DOCKER_CONTEXT", "DOCKER_TLS_VERIFY"],
            ),
            (
                Engine::Podman,
                Target::PodmanConnection {
                    name: "saved".into(),
                    uri: "unix:///saved/podman.sock".into(),
                    identity: String::new(),
                },
                vec!["CONTAINER_HOST", "CONTAINER_CONNECTION", "CONTAINER_SSHKEY"],
            ),
        ] {
            let command = ContainerRuntime::new(engine)
                .with_target(target)
                .cli_command(&["ps"]);
            for variable in variables {
                assert!(
                    command
                        .get_envs()
                        .any(|(name, value)| name == variable && value.is_none())
                );
            }
        }
    }

    #[test]
    fn only_loopback_machine_connections_are_local() {
        assert!(local_machine_uri(
            "ssh://core@127.0.0.1:1234/run/podman.sock"
        ));
        assert!(local_machine_uri("ssh://root@[::1]:1234/run/podman.sock"));
        assert!(!local_machine_uri(
            "ssh://core@localhost.example:1234/run/podman.sock"
        ));
        assert!(!local_machine_uri("ssh://core@server:1234/run/podman.sock"));
    }
}
