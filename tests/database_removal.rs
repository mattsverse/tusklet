//! Removal and retry behavior without a Docker daemon or real data volumes.
#![cfg(unix)]

mod support;

use std::fs;
use support::write_executable;
use tusklet::{
    model::{Database, DatabaseConfig},
    runtime::ContainerRuntime,
    service::{Action, Service},
    store::Store,
};

struct Fixture {
    directory: tempfile::TempDir,
    service: Service,
    database: Database,
}

impl Fixture {
    fn new(state: Option<&str>) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("docker");
        write_executable(
            &executable,
            r#"#!/bin/sh
set -eu
if [ "$1" = context ]; then
    if [ "$2" = show ]; then echo test; else echo unix:///test/docker.sock; fi
    exit 0
fi
if [ "$1" = --host ]; then shift 2; fi
cd "$(dirname "$0")"
printf '%s\n' "$*" >> commands
case "$1 $2" in
    'ps --all')
        if [ -f states ]; then cat states; fi
        ;;
    'inspect --format')
        cat owner
        ;;
    'rm --volumes')
        if [ -f fail-container ]; then echo 'Container removal failed' >&2; exit 1; fi
        rm -f states
        ;;
    'volume ls')
        if [ -f fail-list ]; then echo 'Volume listing failed' >&2; exit 1; fi
        cat volumes
        ;;
    'volume rm')
        if [ -f fail-volume ]; then echo 'Volume is in use' >&2; exit 1; fi
        test "$#" -eq 3
        test "$3" = "$(cat volume-name)"
        : > volumes
        touch volume-removed
        ;;
    *)
        echo 'Unexpected Docker command' >&2
        exit 1
        ;;
esac
"#,
        );
        let service = Service {
            store: Store::in_memory().unwrap(),
            runtime: ContainerRuntime::with_executable(
                tusklet::runtime::Engine::Docker,
                executable,
            ),
        };
        let project = service.store.create_project("Removal tests").unwrap();
        let id = service
            .store
            .insert_database(
                &project,
                DatabaseConfig {
                    name: "Removal test".into(),
                    ..DatabaseConfig::default()
                },
                15432,
            )
            .unwrap();
        let database = service.store.database(&id).unwrap();
        if let Some(state) = state {
            fs::write(
                directory.path().join("states"),
                serde_json::json!({
                    "Names": database.container_name(), "State": state, "Status": "",
                })
                .to_string(),
            )
            .unwrap();
        }
        fs::write(directory.path().join("owner"), &id).unwrap();
        fs::write(directory.path().join("volume-name"), database.volume_name()).unwrap();
        fs::write(directory.path().join("volumes"), database.volume_name()).unwrap();
        Self {
            directory,
            service,
            database,
        }
    }

    fn remove(&mut self, remove_volume: bool) -> anyhow::Result<(String, Option<String>)> {
        self.service.execute(Action::DeleteDatabase {
            id: self.database.id.clone(),
            remove_volume,
        })
    }

    fn commands(&self) -> Vec<String> {
        fs::read_to_string(self.directory.path().join("commands"))
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn fail(&self, operation: &str) {
        fs::write(self.directory.path().join(format!("fail-{operation}")), "").unwrap();
    }
}

#[test]
fn retaining_data_never_runs_volume_commands() {
    let mut fixture = Fixture::new(Some("exited"));
    let (message, _) = fixture.remove(false).unwrap();
    assert!(message.contains("retained"));
    assert!(
        fixture
            .service
            .store
            .database(&fixture.database.id)
            .is_err()
    );
    assert!(!fixture.directory.path().join("states").exists());
    assert!(
        !fixture
            .commands()
            .iter()
            .any(|command| command.starts_with("volume "))
    );
    assert!(!fixture.directory.path().join("volume-removed").exists());
}

#[test]
fn opt_in_deletes_only_the_database_volume_after_its_container() {
    let mut fixture = Fixture::new(Some("exited"));
    let (message, _) = fixture.remove(true).unwrap();
    assert!(message.contains("deleted"));
    assert!(
        fixture
            .service
            .store
            .database(&fixture.database.id)
            .is_err()
    );
    let commands = fixture.commands();
    let container = commands
        .iter()
        .position(|command| {
            command == &format!("rm --volumes {}", fixture.database.container_name())
        })
        .unwrap();
    let volume = commands
        .iter()
        .position(|command| command == &format!("volume rm {}", fixture.database.volume_name()))
        .unwrap();
    assert!(container < volume);
    assert!(fixture.directory.path().join("volume-removed").exists());
}

#[test]
fn absent_volume_is_allowed_and_similarly_named_volumes_are_untouched() {
    for similar_volume in [false, true] {
        let mut fixture = Fixture::new(None);
        let volumes = if similar_volume {
            format!("unrelated-volume\n{}-other", fixture.database.volume_name())
        } else {
            String::new()
        };
        fs::write(fixture.directory.path().join("volumes"), volumes).unwrap();
        fixture.remove(true).unwrap();
        assert!(
            fixture
                .service
                .store
                .database(&fixture.database.id)
                .is_err()
        );
        assert!(
            !fixture
                .commands()
                .iter()
                .any(|command| command.starts_with("volume rm "))
        );
    }
}

#[test]
fn failed_volume_removal_keeps_metadata_and_can_be_retried_without_a_container() {
    let mut fixture = Fixture::new(Some("exited"));
    fixture.fail("volume");
    let error = fixture.remove(true).unwrap_err();
    assert!(format!("{error:#}").contains("entry has been kept"));
    assert!(!fixture.directory.path().join("states").exists());
    assert!(fixture.service.store.database(&fixture.database.id).is_ok());
    assert!(!fixture.directory.path().join("volume-removed").exists());

    fs::remove_file(fixture.directory.path().join("fail-volume")).unwrap();
    fixture.remove(true).unwrap();
    assert!(
        fixture
            .service
            .store
            .database(&fixture.database.id)
            .is_err()
    );
    assert!(fixture.directory.path().join("volume-removed").exists());
}

#[test]
fn failed_volume_listing_does_not_treat_the_volume_as_absent() {
    let mut fixture = Fixture::new(None);
    fixture.fail("list");
    assert!(fixture.remove(true).is_err());
    assert!(fixture.service.store.database(&fixture.database.id).is_ok());
    assert!(!fixture.directory.path().join("volume-removed").exists());
}

#[test]
fn active_container_and_failed_container_removal_protect_volume_and_metadata() {
    for state in ["running", "exited"] {
        let mut fixture = Fixture::new(Some(state));
        if state == "exited" {
            fixture.fail("container");
        }
        assert!(fixture.remove(true).is_err());
        assert!(fixture.service.store.database(&fixture.database.id).is_ok());
        assert!(
            !fixture
                .commands()
                .iter()
                .any(|command| command.starts_with("volume "))
        );
    }
}

#[test]
fn ownership_mismatch_blocks_both_container_and_volume_removal() {
    let mut fixture = Fixture::new(Some("exited"));
    fs::write(fixture.directory.path().join("owner"), "another-database").unwrap();
    assert!(fixture.remove(true).is_err());
    assert!(fixture.service.store.database(&fixture.database.id).is_ok());
    assert!(fixture.directory.path().join("states").exists());
    assert!(
        !fixture
            .commands()
            .iter()
            .any(|command| command.starts_with("volume "))
    );
}
