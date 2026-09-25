use crate::{
    docker::Docker,
    model::{ContainerState, DatabaseConfig, DatabaseView, Snapshot},
    store::Store,
};
use anyhow::{Context, Result, ensure};
use std::{
    net::{Ipv4Addr, TcpListener},
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::Duration,
};

pub struct Service {
    pub store: Store,
    pub docker: Docker,
}

#[derive(Clone)]
pub enum Action {
    CreateProject(String),
    RenameProject(String, String),
    DeleteProject(String),
    CreateDatabase(String, DatabaseConfig),
    UpdateDatabase(String, DatabaseConfig),
    Start(String, bool),
    Stop(String),
    DeleteDatabase(String),
    Dump(String, PathBuf),
    Restore(String, PathBuf),
}

pub enum Request {
    Select(Option<String>, bool),
    Execute(Action),
    Refresh,
}

pub enum Event {
    Snapshot(Box<Snapshot>, Option<String>),
    Completed(Result<String, String>, Option<String>),
    Fatal(String),
}

impl Service {
    /// Load workspace metadata and optionally the selected database logs.
    /// Docker failures are included in the snapshot so offline metadata remains usable.
    ///
    /// # Errors
    /// Returns an error if persisted metadata cannot be read or decoded.
    pub fn snapshot(&self, selected: Option<&str>, follow: bool) -> Result<Snapshot> {
        let mut snapshot = Snapshot {
            projects: self.store.projects()?,
            ..Default::default()
        };
        let states = self.docker.states();
        if let Err(error) = &states {
            snapshot.docker_error = Some(format!("{error:#}"));
        }
        snapshot.databases = self
            .store
            .databases()?
            .into_iter()
            .map(|database| {
                let state = match &states {
                    Ok(states) => states
                        .get(&database.container_name())
                        .cloned()
                        .unwrap_or_default(),
                    Err(_) => ContainerState::Unknown,
                };
                DatabaseView { database, state }
            })
            .collect();
        if states.is_ok() {
            match self.docker.local_images() {
                Ok(images) => snapshot.images = images,
                Err(error) => snapshot.docker_error = Some(format!("{error:#}")),
            }
            if follow
                && let Some(db) = snapshot
                    .databases
                    .iter()
                    .find(|db| Some(db.database.id.as_str()) == selected)
                && db.state != ContainerState::NotCreated
            {
                snapshot.logs = self
                    .docker
                    .logs(&db.database)
                    .unwrap_or_else(|error| format!("Could not read logs: {error:#}"));
            }
        }
        Ok(snapshot)
    }

    /// Apply one user operation and return a message and optional database selection.
    ///
    /// # Errors
    /// Rejects invalid configuration, conflicting ports, unavailable resources,
    /// and unsafe state transitions; propagates storage and Docker failures.
    pub fn execute(&self, action: Action) -> Result<(String, Option<String>)> {
        match action {
            Action::CreateProject(name) => {
                self.store.create_project(&name)?;
                Ok(("Project created.".into(), None))
            }
            Action::RenameProject(id, name) => {
                self.store.rename_project(&id, &name)?;
                Ok(("Project renamed.".into(), None))
            }
            Action::DeleteProject(id) => {
                self.store.delete_project(&id)?;
                Ok(("Project removed.".into(), None))
            }
            Action::CreateDatabase(project, config) => {
                config.validate()?;
                let port = allocate_port(config.reserved_port, &self.store.allocated_ports()?)?;
                let id = self.store.insert_database(&project, config, port)?;
                Ok((
                    "Database created. Start it when you're ready.".into(),
                    Some(id),
                ))
            }
            Action::UpdateDatabase(id, config) => {
                config.validate()?;
                let db = self.store.database(&id)?;
                ensure!(
                    config.tag == db.config.tag
                        && config.database == db.config.database
                        && config.user == db.config.user,
                    "Create a new database to change the image tag, initial database, or user."
                );
                let state = self
                    .docker
                    .states()?
                    .get(&db.container_name())
                    .cloned()
                    .unwrap_or_default();
                ensure!(
                    !state.is_active(),
                    "Stop the database before editing its settings."
                );
                let port = match config.reserved_port {
                    Some(port) if port != db.port => {
                        allocate_port(Some(port), &self.store.allocated_ports()?)?
                    }
                    _ => db.port,
                };
                // Removing a stopped container preserves its named volume. Failure to
                // save metadata leaves the old settings available for the next start.
                self.docker.remove_container(&db)?;
                self.store.update_database(&id, config, port)?;
                Ok((
                    "Settings saved. Start the database to apply them.".into(),
                    Some(id),
                ))
            }
            Action::Start(id, allow_pull) => {
                let db = self.store.database(&id)?;
                let state = self
                    .docker
                    .states()?
                    .get(&db.container_name())
                    .cloned()
                    .unwrap_or_default();
                if !state.is_active() {
                    TcpListener::bind((Ipv4Addr::LOCALHOST, db.port))
                        .with_context(|| format!("Port {} is in use by another application. Stop that application or edit the database's port.", db.port))?;
                }
                self.docker.start(&db, allow_pull)?;
                Ok((
                    "Container started. PostgreSQL is initializing.".into(),
                    Some(id),
                ))
            }
            Action::Stop(id) => {
                self.docker.stop(&self.store.database(&id)?)?;
                Ok(("Database stopped. Your data is preserved.".into(), Some(id)))
            }
            Action::DeleteDatabase(id) => {
                let db = self.store.database(&id)?;
                self.docker.remove_container(&db)?;
                self.store.delete_database(&id)?;
                Ok((
                    format!(
                        "Database removed. Data volume {} was retained.",
                        db.volume_name()
                    ),
                    None,
                ))
            }
            Action::Dump(id, path) => {
                self.docker.dump(&self.store.database(&id)?, &path)?;
                Ok((format!("Backup saved to {}", path.display()), Some(id)))
            }
            Action::Restore(id, path) => {
                self.docker.restore(&self.store.database(&id)?, &path)?;
                Ok(("Backup restored successfully.".into(), Some(id)))
            }
        }
    }
}

/// Choose a locally available port that is not reserved by another database.
///
/// # Errors
/// Fails if a requested port is invalid, occupied, or reserved, or if no free
/// automatic port can be found. Docker remains the authority at bind time.
pub fn allocate_port(requested: Option<u16>, allocated: &[u16]) -> Result<u16> {
    if let Some(port) = requested {
        ensure!(port >= 1024, "Port must be between 1024 and 65535.");
        ensure!(
            !allocated.contains(&port),
            "Port {port} is reserved by another Tusklet database."
        );
        TcpListener::bind((Ipv4Addr::LOCALHOST, port))
            .with_context(|| format!("Port {port} is already in use."))?;
        return Ok(port);
    }
    for _ in 0..100 {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let port = listener.local_addr()?.port();
        if port >= 1024 && !allocated.contains(&port) {
            return Ok(port);
        }
    }
    anyhow::bail!("Could not find an available port. Choose one explicitly.")
}

#[must_use]
pub fn spawn_worker(path: PathBuf) -> (Sender<Request>, Receiver<Event>) {
    let (requests, incoming) = mpsc::channel();
    let (events, receiver) = mpsc::channel();
    thread::spawn(move || {
        let store = match Store::open(&path) {
            Ok(store) => store,
            Err(error) => {
                let _ = events.send(Event::Fatal(format!(
                    "Could not open {}: {error:#}",
                    path.display()
                )));
                return;
            }
        };
        let service = Service {
            store,
            docker: Docker::default(),
        };
        let mut selected = None;
        let mut follow = true;
        let mut first = true;
        loop {
            if !first {
                match incoming.recv_timeout(Duration::from_secs(2)) {
                    Ok(Request::Select(id, enabled)) => {
                        selected = id;
                        follow = enabled;
                    }
                    Ok(Request::Execute(action)) => {
                        let result = service.execute(action);
                        let selection = result.as_ref().ok().and_then(|(_, id)| id.clone());
                        if selection.is_some() {
                            selected.clone_from(&selection);
                        }
                        let result = result
                            .map(|(message, _)| message)
                            .map_err(|error| format!("{error:#}"));
                        // Send refreshed metadata before completing the action so a
                        // newly created database can be selected without a race.
                        if let Ok(snapshot) = service.snapshot(selected.as_deref(), follow) {
                            let _ =
                                events.send(Event::Snapshot(Box::new(snapshot), selected.clone()));
                        }
                        if events.send(Event::Completed(result, selection)).is_err() {
                            break;
                        }
                        continue;
                    }
                    Ok(Request::Refresh) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
            first = false;
            match service.snapshot(selected.as_deref(), follow) {
                Ok(snapshot) => {
                    if events
                        .send(Event::Snapshot(Box::new(snapshot), selected.clone()))
                        .is_err()
                    {
                        break;
                    }
                }
                Err(error) => {
                    if events.send(Event::Fatal(format!("{error:#}"))).is_err() {
                        break;
                    }
                }
            }
        }
    });
    (requests, receiver)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn workspace_metadata_remains_available_without_docker() {
        let store = Store::in_memory().unwrap();
        let project = store.create_project("Offline project").unwrap();
        store
            .insert_database(
                &project,
                DatabaseConfig {
                    name: "Saved database".into(),
                    ..Default::default()
                },
                15432,
            )
            .unwrap();
        let service = Service {
            store,
            docker: Docker::with_executable(PathBuf::from("/__tusklet_missing_docker__")),
        };
        let snapshot = service.snapshot(None, true).unwrap();
        assert_eq!(snapshot.projects[0].name, "Offline project");
        assert_eq!(snapshot.databases[0].database.config.name, "Saved database");
        assert_eq!(snapshot.databases[0].state, ContainerState::Unknown);
        assert!(snapshot.docker_error.is_some());
    }

    #[test]
    fn ports_respect_application_reservations_and_other_processes() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let occupied = listener.local_addr().unwrap().port();
        assert!(allocate_port(Some(occupied), &[]).is_err());
        let available = allocate_port(None, &[occupied]).unwrap();
        assert_ne!(available, occupied);
        assert!(allocate_port(Some(available), &[available]).is_err());
        assert!(allocate_port(Some(80), &[]).is_err());
    }
}
