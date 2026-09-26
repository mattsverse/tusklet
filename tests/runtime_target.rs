//! Runtime ownership stays fixed across CLI default changes and application restarts.
#![cfg(unix)]

use std::{fs, os::unix::fs::PermissionsExt};
use tusklet::{
    model::DatabaseConfig,
    runtime::{ContainerRuntime, Engine, RuntimeSettings, Target},
    service::{Action, Service},
    store::Store,
};

const DOCKER_SCRIPT: &str = r#"#!/bin/sh
set -eu
cd "$(dirname "$0")"
if [ "$1" = context ]; then
    echo discovery >> calls
    if [ "$2" = show ]; then echo test-context; else cat current-host; fi
    exit 0
fi
test "$1" = --host
printf '%s\n' "$2" >> targets
shift 2
case "$1" in
    ps)
        if [ -f unavailable ]; then echo 'Original runtime unavailable' >&2; exit 1; fi
        ;;
    image) ;;
    *) echo 'Unexpected mutation' >&2; exit 1 ;;
esac
"#;

#[test]
fn saved_docker_target_survives_default_changes_restarts_and_outages() {
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("docker");
    fs::write(&executable, DOCKER_SCRIPT).unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(
        directory.path().join("current-host"),
        "unix:///original.sock",
    )
    .unwrap();
    let path = directory.path().join("workspace.sqlite3");
    {
        let mut service = Service {
            store: Store::open(&path).unwrap(),
            runtime: ContainerRuntime::with_executable(Engine::Docker, executable.clone()),
        };
        assert!(
            service
                .snapshot(None, false)
                .unwrap()
                .runtime_error
                .is_none()
        );
        let project = service.store.create_project("Test").unwrap();
        service
            .store
            .insert_database(
                &project,
                DatabaseConfig {
                    name: "Saved".into(),
                    ..Default::default()
                },
                5432,
            )
            .unwrap();
        assert!(
            service
                .execute(Action::SelectRuntime(Engine::Podman))
                .is_err()
        );
    }
    fs::write(
        directory.path().join("current-host"),
        "unix:///different.sock",
    )
    .unwrap();
    let mut service = Service {
        store: Store::open(&path).unwrap(),
        runtime: ContainerRuntime::with_executable(Engine::Docker, executable),
    };
    assert!(
        service
            .snapshot(None, false)
            .unwrap()
            .runtime_error
            .is_none()
    );
    fs::write(directory.path().join("unavailable"), "").unwrap();
    let snapshot = service.snapshot(None, false).unwrap();
    assert!(
        snapshot
            .runtime_error
            .unwrap()
            .contains("Original runtime unavailable")
    );
    assert_eq!(snapshot.databases.len(), 1);
    assert_eq!(snapshot.runtime_engine, Engine::Docker);
    assert_eq!(
        fs::read_to_string(directory.path().join("calls"))
            .unwrap()
            .lines()
            .count(),
        2
    );
    assert!(
        fs::read_to_string(directory.path().join("targets"))
            .unwrap()
            .lines()
            .all(|host| host == "unix:///original.sock")
    );
}

#[test]
fn native_podman_refuses_a_changed_storage_location() {
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("podman");
    fs::write(
        &executable,
        r#"#!/bin/sh
set -eu
while [ "$1" = --remote=false ]; do shift; done
test "$1" = info
echo /different/storage
"#,
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let store = Store::in_memory().unwrap();
    store
        .save_runtime(
            &RuntimeSettings::default(),
            RuntimeSettings {
                engine: Engine::Podman,
                target: Some(Target::PodmanLocal {
                    graph_root: "/original/storage".into(),
                }),
            },
        )
        .unwrap();
    let mut service = Service {
        store,
        runtime: ContainerRuntime::with_executable(Engine::Podman, executable),
    };
    let snapshot = service.snapshot(None, false).unwrap();
    assert!(
        snapshot
            .runtime_error
            .unwrap()
            .contains("storage location changed")
    );
    assert_eq!(snapshot.runtime_engine, Engine::Podman);
}
