//! Podman output, noninteractive image handling, and transfer contracts without a VM.
#![cfg(unix)]

mod support;

use std::fs;
use support::write_executable;
use tusklet::{
    model::{ContainerState, Database, DatabaseConfig},
    runtime::{ContainerRuntime, Engine, Target},
};

fn runtime(script: &str) -> (tempfile::TempDir, ContainerRuntime) {
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("podman");
    write_executable(
        &executable,
        &format!("#!/bin/sh\nset -eu\ncd \"$(dirname \"$0\")\"\n{script}\n"),
    );
    (
        directory,
        ContainerRuntime::with_executable(Engine::Podman, executable),
    )
}

fn database() -> Database {
    Database {
        id: "test".into(),
        project_id: "project".into(),
        config: DatabaseConfig {
            name: "Test".into(),
            ..Default::default()
        },
        password: "private-test-password".into(),
        port: 15432,
    }
}

#[test]
fn target_discovery_resolves_native_storage_or_the_default_local_machine() {
    let (_directory, runtime) = runtime(
        r#"
case "$1" in
    --remote=false) echo /local/podman/storage ;;
    system)
        printf '%s\n' '[{"Name":"other","URI":"ssh://core@127.0.0.1:1111/run/podman.sock","Identity":"/other/key","Default":false,"IsMachine":true},{"Name":"selected","URI":"ssh://core@127.0.0.1:2222/run/podman.sock","Identity":"/selected/key","Default":true,"IsMachine":true}]'
        ;;
    *) exit 1 ;;
esac
"#,
    );
    let target = runtime.resolve_target().unwrap();
    if cfg!(target_os = "linux") {
        assert_eq!(
            target,
            Target::PodmanLocal {
                graph_root: "/local/podman/storage".into()
            }
        );
    } else {
        assert_eq!(
            target,
            Target::PodmanConnection {
                name: "selected".into(),
                uri: "ssh://core@127.0.0.1:2222/run/podman.sock".into(),
                identity: "/selected/key".into(),
            }
        );
    }
}

#[test]
fn inspection_normalizes_health_and_never_treats_paused_or_unknown_as_stopped() {
    let (_directory, runtime) = runtime(
        r#"
case "$1" in
    ps) echo container-id ;;
    inspect)
        test "$2 $3" = '--type container'
        printf '%s\n' \
          '{"Name":"/ready","State":{"Status":"running","Health":{"Status":"healthy"}}}' \
          '{"Name":"unhealthy","State":{"Status":"running","Healthcheck":{"Status":"unhealthy"}}}' \
          '{"Name":"starting","State":{"Status":"running","Healthcheck":{"Status":"starting"}}}' \
          '{"Name":"unchecked","State":{"Status":"running","Healthcheck":{"Status":""}}}' \
          '{"Name":"stopped","State":{"Status":"stopped"}}' \
          '{"Name":"initialized","State":{"Status":"initialized"}}' \
          '{"Name":"paused","State":{"Status":"paused"}}' \
          '{"Name":"future","State":{"Status":"future-state"}}'
        ;;
    *) exit 1 ;;
esac
echo 'Podman diagnostic' >&2
"#,
    );
    let states = runtime.states().unwrap();
    assert_eq!(states["ready"], ContainerState::Running);
    assert_eq!(states["unhealthy"], ContainerState::Unhealthy);
    assert_eq!(states["starting"], ContainerState::Starting);
    assert_eq!(states["unchecked"], ContainerState::Starting);
    assert_eq!(states["stopped"], ContainerState::Stopped);
    assert_eq!(states["initialized"], ContainerState::Stopped);
    assert_eq!(states["paused"], ContainerState::Unknown);
    assert_eq!(states["future"], ContainerState::Unknown);
}

#[test]
fn cached_official_images_start_offline_with_podman_logging_and_private_password() {
    let (directory, runtime) = runtime(
        r#"
case "$1" in
    ps) ;;
    image)
        printf '%s\n' 'docker.io/library/postgres:18' 'postgres:18' \
          'docker.io/postgres:17-alpine' 'example.com/library/postgres:99' \
          'localhost/postgres:99' 'docker.io/library/postgres:<none>'
        ;;
    create)
        test "$POSTGRES_PASSWORD" = private-test-password
        printf '%s\n' "$@" > create-args
        ;;
    inspect) echo test ;;
    start) touch started ;;
    *) echo 'Unexpected command, including any pull' >&2; exit 1 ;;
esac
"#,
    );
    assert_eq!(runtime.local_images().unwrap(), ["17-alpine", "18"]);
    runtime.start(&database()).unwrap();
    let args = fs::read_to_string(directory.path().join("create-args")).unwrap();
    assert!(args.contains("docker.io/library/postgres:18"));
    assert!(args.contains("k8s-file\n"));
    assert!(args.contains("max-size=10mb\n"));
    assert!(!args.contains("max-file"));
    assert!(!args.contains("private-test-password"));
    assert!(directory.path().join("started").exists());
}

#[test]
fn unknown_container_blocks_start_and_removal() {
    let (directory, runtime) = runtime(
        r#"
case "$1" in
    ps) echo container-id ;;
    inspect) echo '{"Name":"tusklet-test","State":{"Status":"paused"}}' ;;
    *) touch mutated; exit 1 ;;
esac
"#,
    );
    assert!(runtime.start(&database()).is_err());
    assert!(runtime.remove_container(&database()).is_err());
    assert!(!directory.path().join("mutated").exists());
}

#[test]
fn pinned_machine_streams_binary_backups_and_restores_without_a_tty() {
    let (directory, runtime) = runtime(
        r#"
test "$1" = --url
test "$2" = ssh://core@127.0.0.1:1234/run/user/1000/podman.sock
shift 2
test "$1 $2" = '--identity /test/identity'
shift 2
case "$1" in
    inspect) echo test ;;
    exec)
        if [ "$2" = --interactive ]; then
            test "$4" = pg_restore
            cat > restored
        else
            test "$3" = pg_dump
            printf 'PGDMP\000\377\n'
        fi
        ;;
    *) exit 1 ;;
esac
echo warning >&2
"#,
    );
    let runtime = runtime.with_target(Target::PodmanConnection {
        name: "machine".into(),
        uri: "ssh://core@127.0.0.1:1234/run/user/1000/podman.sock".into(),
        identity: "/test/identity".into(),
    });
    let dump = directory.path().join("backup.dump");
    runtime.dump(&database(), &dump).unwrap();
    assert_eq!(fs::read(&dump).unwrap(), b"PGDMP\x00\xff\n");
    runtime.restore(&database(), &dump).unwrap();
    assert_eq!(
        fs::read(directory.path().join("restored")).unwrap(),
        fs::read(dump).unwrap()
    );
}
