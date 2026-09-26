//! Opt-in tests. Only uniquely named test containers/volumes are changed.
use std::{
    net::TcpStream,
    thread,
    time::{Duration, Instant},
};
use tusklet::{
    model::{ContainerState, DatabaseConfig},
    runtime::{ContainerRuntime, Engine},
    service::{Action, Service},
    store::Store,
};

struct Cleanup {
    runtime: ContainerRuntime,
    names: Vec<(String, String)>,
}
impl Drop for Cleanup {
    fn drop(&mut self) {
        for (container, volume) in &self.names {
            let _ = self
                .runtime
                .cli_command(&["rm", "--force", "--volumes", container])
                .output();
            let _ = self.runtime.cli_command(&["volume", "rm", volume]).output();
        }
    }
}

fn sql(runtime: &ContainerRuntime, container: &str, query: &str) -> String {
    let output = runtime
        .cli_command(&[
            "exec",
            container,
            "psql",
            "-X",
            "-U",
            "postgres",
            "-d",
            "postgres",
            "-At",
            "-v",
            "ON_ERROR_STOP=1",
            "-c",
            query,
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "SQL failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().into()
}

fn volume_exists(runtime: &ContainerRuntime, name: &str) -> bool {
    runtime
        .cli_command(&["volume", "inspect", "--format", "{{.Name}}", name])
        .output()
        .unwrap()
        .status
        .success()
}

fn wait_ready(runtime: &ContainerRuntime, container: &str) {
    let start = Instant::now();
    loop {
        if runtime.states().unwrap().get(container) == Some(&ContainerState::Running) {
            return;
        }
        assert!(
            start.elapsed() < Duration::from_secs(90),
            "PostgreSQL did not become ready"
        );
        thread::sleep(Duration::from_millis(500));
    }
}

#[test]
#[ignore = "requires Docker or Podman and cached official PostgreSQL 17/18 Alpine images"]
fn offline_lifecycle_persistence_dump_restore_and_port_reservations() {
    let mut service = prepared_service();
    let project = service.store.create_project("Integration test").unwrap();
    let mut cleanup = Cleanup {
        runtime: service.runtime.clone(),
        names: Vec::new(),
    };
    for tag in ["17-alpine", "18-alpine"] {
        let remove_volume = tag == "18-alpine";
        let config = DatabaseConfig {
            name: format!("Smoke {tag}"),
            tag: tag.into(),
            command: "-c wal_level=logical".into(),
            ..Default::default()
        };
        let (_, id) = service
            .execute(Action::CreateDatabase(project.clone(), config))
            .unwrap();
        let id = id.unwrap();
        let db = service.store.database(&id).unwrap();
        let name = db.container_name();
        cleanup.names.push((name.clone(), db.volume_name()));
        service.execute(Action::Start(id.clone())).unwrap();
        wait_ready(&service.runtime, &name);
        TcpStream::connect(("127.0.0.1", db.port))
            .expect("database is reachable from the desktop host");
        assert_eq!(sql(&service.runtime, &name, "SHOW wal_level"), "logical");
        sql(
            &service.runtime,
            &name,
            "CREATE TABLE tusklet_test(value TEXT); INSERT INTO tusklet_test VALUES ('persisted');",
        );
        assert!(!service.runtime.logs(&db).unwrap().is_empty());
        verify_backups(&mut service, &id, &name);
        service.execute(Action::Stop(id.clone())).unwrap();
        let mut updated = db.config.clone();
        updated.command = "-c wal_level=logical -c max_replication_slots=12".into();
        service
            .execute(Action::UpdateDatabase(id.clone(), updated))
            .unwrap();
        service.execute(Action::Start(id.clone())).unwrap();
        wait_ready(&service.runtime, &name);
        TcpStream::connect(("127.0.0.1", db.port))
            .expect("database is reachable from the desktop host");
        assert_eq!(
            sql(&service.runtime, &name, "SELECT count(*) FROM tusklet_test"),
            "2",
            "Data must survive container recreation on both PG17 and PG18"
        );
        assert_eq!(
            sql(&service.runtime, &name, "SHOW max_replication_slots"),
            "12"
        );
        assert_eq!(service.store.database(&id).unwrap().port, db.port);
        assert!(
            service
                .execute(Action::DeleteDatabase {
                    id: id.clone(),
                    remove_volume,
                })
                .is_err(),
            "Cannot remove an active database"
        );
        service.execute(Action::Stop(id.clone())).unwrap();
        service
            .execute(Action::DeleteDatabase {
                id: id.clone(),
                remove_volume,
            })
            .unwrap();
        assert!(service.store.database(&id).is_err());
        assert_eq!(
            volume_exists(&service.runtime, &db.volume_name()),
            !remove_volume,
            "Removal must honor the volume deletion choice"
        );
    }
}

fn prepared_service() -> Service {
    let mut service = Service {
        store: Store::in_memory().unwrap(),
        runtime: ContainerRuntime::default(),
    };
    let engine = match std::env::var("TUSKLET_TEST_RUNTIME").as_deref() {
        Ok("podman") => Engine::Podman,
        Ok("docker") | Err(_) => Engine::Docker,
        Ok(other) => panic!("Unknown test runtime: {other}"),
    };
    service.execute(Action::SelectRuntime(engine)).unwrap();
    let snapshot = service.snapshot(None, false).unwrap();
    assert!(
        snapshot.runtime_error.is_none(),
        "{:?}",
        snapshot.runtime_error
    );
    for tag in ["17-alpine", "18-alpine"] {
        assert!(
            snapshot.images.iter().any(|image| image == tag),
            "Cache {tag} before running integration tests"
        );
    }
    service
}

fn verify_backups(service: &mut Service, id: &str, name: &str) {
    let directory = tempfile::tempdir().unwrap();
    let backup = directory.path().join("test.dump");
    service
        .execute(Action::Dump(id.to_owned(), backup.clone()))
        .unwrap();
    assert!(std::fs::metadata(&backup).unwrap().len() > 5);
    assert!(
        service
            .execute(Action::Dump(id.to_owned(), backup.clone()))
            .is_err(),
        "Must not overwrite an existing backup"
    );
    sql(&service.runtime, name, "DROP TABLE tusklet_test");
    service
        .execute(Action::Restore(id.to_owned(), backup))
        .unwrap();
    assert_eq!(
        sql(&service.runtime, name, "SELECT value FROM tusklet_test"),
        "persisted"
    );
    let plain = directory.path().join("plain.sql");
    std::fs::write(&plain, "INSERT INTO tusklet_test VALUES ('plain SQL');").unwrap();
    service
        .execute(Action::Restore(id.to_owned(), plain))
        .unwrap();
    let broken = directory.path().join("broken.sql");
    std::fs::write(
        &broken,
        "INSERT INTO tusklet_test VALUES ('rolled back'); SELECT nonexistent_column;",
    )
    .unwrap();
    assert!(
        service
            .execute(Action::Restore(id.to_owned(), broken))
            .is_err()
    );
    assert_eq!(
        sql(&service.runtime, name, "SELECT count(*) FROM tusklet_test"),
        "2"
    );
}
