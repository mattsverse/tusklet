//! Opt-in tests. Only uniquely named test containers/volumes are changed.
use std::{
    process::Command,
    thread,
    time::{Duration, Instant},
};
use tusklet::{
    docker::Docker,
    model::{ContainerState, DatabaseConfig},
    service::{Action, Service},
    store::Store,
};

#[derive(Default)]
struct Cleanup {
    names: Vec<(String, String)>,
}
impl Drop for Cleanup {
    fn drop(&mut self) {
        for (container, volume) in &self.names {
            let _ = Command::new("docker")
                .args(["rm", "--force", "--volumes", container])
                .output();
            let _ = Command::new("docker")
                .args(["volume", "rm", volume])
                .output();
        }
    }
}

fn sql(container: &str, query: &str) -> String {
    let output = Command::new("docker")
        .args([
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

fn wait_ready(docker: &Docker, container: &str) {
    let start = Instant::now();
    loop {
        if docker.states().unwrap().get(container) == Some(&ContainerState::Running) {
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
#[ignore = "requires a local Docker daemon and cached postgres:17-alpine and postgres:18-alpine images"]
fn offline_lifecycle_persistence_dump_restore_and_port_reservations() {
    let service = Service {
        store: Store::in_memory().unwrap(),
        docker: Docker::default(),
    };
    let project = service.store.create_project("Integration test").unwrap();
    let mut cleanup = Cleanup::default();
    for tag in ["17-alpine", "18-alpine"] {
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
        service.execute(Action::Start(id.clone(), false)).unwrap();
        wait_ready(&service.docker, &name);
        assert_eq!(sql(&name, "SHOW wal_level"), "logical");
        sql(
            &name,
            "CREATE TABLE tusklet_test(value TEXT); INSERT INTO tusklet_test VALUES ('persisted');",
        );
        assert!(!service.docker.logs(&db).unwrap().is_empty());
        let directory = tempfile::tempdir().unwrap();
        let backup = directory.path().join("test.dump");
        service
            .execute(Action::Dump(id.clone(), backup.clone()))
            .unwrap();
        assert!(std::fs::metadata(&backup).unwrap().len() > 5);
        assert!(
            service
                .execute(Action::Dump(id.clone(), backup.clone()))
                .is_err(),
            "Must not overwrite an existing backup"
        );
        sql(&name, "DROP TABLE tusklet_test");
        service
            .execute(Action::Restore(id.clone(), backup))
            .unwrap();
        assert_eq!(sql(&name, "SELECT value FROM tusklet_test"), "persisted");
        let plain = directory.path().join("plain.sql");
        std::fs::write(&plain, "INSERT INTO tusklet_test VALUES ('plain SQL');").unwrap();
        service.execute(Action::Restore(id.clone(), plain)).unwrap();
        let broken = directory.path().join("broken.sql");
        std::fs::write(
            &broken,
            "INSERT INTO tusklet_test VALUES ('rolled back'); SELECT nonexistent_column;",
        )
        .unwrap();
        assert!(
            service
                .execute(Action::Restore(id.clone(), broken))
                .is_err()
        );
        assert_eq!(sql(&name, "SELECT count(*) FROM tusklet_test"), "2");
        service.execute(Action::Stop(id.clone())).unwrap();
        let mut updated = db.config.clone();
        updated.command = "-c wal_level=logical -c max_replication_slots=12".into();
        service
            .execute(Action::UpdateDatabase(id.clone(), updated))
            .unwrap();
        service.execute(Action::Start(id.clone(), false)).unwrap();
        wait_ready(&service.docker, &name);
        assert_eq!(
            sql(&name, "SELECT count(*) FROM tusklet_test"),
            "2",
            "Data must survive container recreation on both PG17 and PG18"
        );
        assert_eq!(sql(&name, "SHOW max_replication_slots"), "12");
        assert_eq!(service.store.database(&id).unwrap().port, db.port);
        assert!(
            service.execute(Action::DeleteDatabase(id.clone())).is_err(),
            "Cannot remove an active database"
        );
        service.execute(Action::Stop(id.clone())).unwrap();
        service.execute(Action::DeleteDatabase(id.clone())).unwrap();
        assert!(service.store.database(&id).is_err());
        assert!(
            Command::new("docker")
                .args([
                    "volume",
                    "inspect",
                    "--format",
                    "{{.Name}}",
                    &db.volume_name()
                ])
                .output()
                .unwrap()
                .status
                .success(),
            "Removal must preserve the data volume"
        );
    }
}
