//! Exercise automatic image selection without a Docker daemon or registry.
#![cfg(unix)]

use std::{fs, os::unix::fs::PermissionsExt};
use tusklet::{
    docker::Docker,
    model::DatabaseConfig,
    service::{Action, Service},
    store::Store,
};

struct DockerFixture {
    directory: tempfile::TempDir,
    service: Service,
}

impl DockerFixture {
    fn new(images: &str, registry_available: bool) -> Self {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("images"), images).unwrap();
        if registry_available {
            fs::write(directory.path().join("registry-available"), "").unwrap();
        }
        let executable = directory.path().join("docker");
        fs::write(
            &executable,
            r#"#!/bin/sh
set -eu
cd "$(dirname "$0")"
printf '%s\n' "$1" >> commands
case "$1" in
    ps)
        if [ -f states ]; then cat states; fi
        ;;
    image)
        cat images
        ;;
    pull)
        if [ ! -f registry-available ]; then
            echo 'Docker Hub is unreachable' >&2
            exit 1
        fi
        printf '%s\n' "$2" >> images
        ;;
    create)
        touch created
        ;;
    inspect)
        printf '%s\n' "${4#tusklet-}"
        ;;
    start)
        touch started
        ;;
    *)
        echo 'Unexpected Docker command' >&2
        exit 1
        ;;
esac
"#,
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let service = Service {
            store: Store::in_memory().unwrap(),
            docker: Docker::with_executable(executable),
        };
        Self { directory, service }
    }

    fn create_database(&self, tag: &str) -> String {
        let project = self.service.store.create_project("Test project").unwrap();
        self.service
            .execute(Action::CreateDatabase(
                project,
                DatabaseConfig {
                    name: "Test database".into(),
                    tag: tag.into(),
                    ..Default::default()
                },
            ))
            .unwrap()
            .1
            .unwrap()
    }

    fn commands(&self) -> Vec<String> {
        fs::read_to_string(self.directory.path().join("commands"))
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

#[test]
fn cached_tag_is_available_for_creation_and_start_when_registry_is_unreachable() {
    let fixture = DockerFixture::new("postgres:17-alpine\n", false);
    let snapshot = fixture.service.snapshot(None, false).unwrap();
    assert!(snapshot.docker_error.is_none());
    assert_eq!(snapshot.images, ["17-alpine"]);

    let id = fixture.create_database(&snapshot.images[0]);
    fixture.service.execute(Action::Start(id)).unwrap();

    assert!(fixture.directory.path().join("created").exists());
    assert!(fixture.directory.path().join("started").exists());
    assert!(!fixture.commands().iter().any(|command| command == "pull"));
}

#[test]
fn missing_tag_is_downloaded_automatically_before_starting() {
    let fixture = DockerFixture::new("postgres:17-alpine\n", true);
    let id = fixture.create_database("18-alpine");
    fixture.service.execute(Action::Start(id)).unwrap();

    assert!(fixture.directory.path().join("started").exists());
    assert!(fixture.commands().iter().any(|command| command == "pull"));
    assert_eq!(
        fixture.service.docker.local_images().unwrap(),
        ["17-alpine", "18-alpine"]
    );
}

#[test]
fn registry_outage_only_blocks_starting_a_missing_tag_not_saving_the_database() {
    let fixture = DockerFixture::new("postgres:17-alpine\n", false);
    let id = fixture.create_database("18-alpine");
    let error = fixture
        .service
        .execute(Action::Start(id.clone()))
        .unwrap_err();
    let message = format!("{error:#}");

    assert!(message.contains("postgres:18-alpine is not available locally"));
    assert!(message.contains("Docker Hub is unreachable"));
    assert!(!fixture.directory.path().join("created").exists());
    assert!(!fixture.directory.path().join("started").exists());
    assert_eq!(
        fixture.service.store.database(&id).unwrap().config.tag,
        "18-alpine"
    );
}

#[test]
fn existing_container_starts_without_a_cached_tag_or_registry_access() {
    let fixture = DockerFixture::new("", false);
    let id = fixture.create_database("18-alpine");
    let database = fixture.service.store.database(&id).unwrap();
    fs::write(
        fixture.directory.path().join("states"),
        serde_json::json!({
            "Names": database.container_name(),
            "State": "exited",
            "Status": "Exited",
        })
        .to_string(),
    )
    .unwrap();

    fixture.service.execute(Action::Start(id)).unwrap();

    assert!(fixture.directory.path().join("started").exists());
    assert!(!fixture.directory.path().join("created").exists());
    assert!(
        fixture
            .commands()
            .iter()
            .all(|command| command != "pull" && command != "image")
    );
}
