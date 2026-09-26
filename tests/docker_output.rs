//! Exercise Docker's output contract without a daemon or registry.
#![cfg(unix)]

use std::{fs, os::unix::fs::PermissionsExt};
use tusklet::{
    docker::Docker,
    model::{ContainerState, Database, DatabaseConfig},
};

fn database() -> Database {
    Database {
        id: "test".into(),
        project_id: "project".into(),
        config: DatabaseConfig {
            name: "Output test".into(),
            ..Default::default()
        },
        password: "output-test-credential".into(),
        port: 15432,
    }
}

fn docker(script: &str) -> (tempfile::TempDir, Docker) {
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("docker");
    fs::write(&executable, format!("#!/bin/sh\nset -eu\n{script}\n")).unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    (directory, Docker::with_executable(executable))
}

#[test]
fn successful_stderr_diagnostics_do_not_corrupt_container_states() {
    let (_directory, docker) = docker(
        r#"
printf '%s\n' '{"Names":"tusklet-test","State":"running","Status":"Up"}'
printf '%s\n' 'Docker warning' >&2
"#,
    );
    assert_eq!(
        docker.states().unwrap()["tusklet-test"],
        ContainerState::Running
    );
}

#[test]
fn successful_stderr_diagnostics_do_not_block_ownership_checks() {
    let (_directory, docker) = docker(
        r#"
case "$1" in
    inspect) printf '%s\n' "${4#tusklet-}" ;;
    stop) printf '%s\n' "$4" ;;
    *) exit 1 ;;
esac
printf '%s\n' 'Docker warning' >&2
"#,
    );
    docker.stop(&database()).unwrap();
}

#[test]
fn logs_preserve_both_output_streams() {
    let (_directory, docker) = docker(
        r"
printf '%s\n' 'stdout log'
printf '%s\n' 'stderr log' >&2
",
    );
    assert_eq!(
        docker.logs(&database()).unwrap(),
        "stdout log\nstderr log\n"
    );
}

#[test]
fn failures_prefer_stderr_and_fall_back_to_stdout() {
    for (script, expected) in [
        (
            "printf 'stdout detail'; printf 'stderr detail' >&2; exit 1",
            "stderr detail",
        ),
        (
            "printf 'stdout detail'; printf ' \\n' >&2; exit 1",
            "stdout detail",
        ),
    ] {
        let (_directory, docker) = docker(script);
        assert_eq!(
            docker.states().unwrap_err().to_string(),
            format!("Docker operation failed: {expected}")
        );
    }
}

#[test]
fn dump_preserves_binary_stdout_without_warning_text_and_never_overwrites() {
    let (directory, docker) = docker(
        r#"
case "$1" in
    inspect) printf '%s\n' "${4#tusklet-}" ;;
    exec) printf 'PGDMP\000\377\n' ;;
    *) exit 1 ;;
esac
printf '%s\n' 'Docker warning' >&2
"#,
    );
    let destination = directory.path().join("backup.dump");
    docker.dump(&database(), &destination).unwrap();
    assert_eq!(fs::read(&destination).unwrap(), b"PGDMP\x00\xff\n");
    assert!(docker.dump(&database(), &destination).is_err());
    assert_eq!(fs::read(&destination).unwrap(), b"PGDMP\x00\xff\n");
}

#[test]
fn failed_dump_does_not_publish_partial_stdout() {
    let (directory, docker) = docker(
        r#"
case "$1" in
    inspect) printf '%s\n' "${4#tusklet-}" ;;
    exec) printf 'partial dump'; printf 'dump failed' >&2; exit 1 ;;
    *) exit 1 ;;
esac
"#,
    );
    let destination = directory.path().join("backup.dump");
    assert!(
        docker
            .dump(&database(), &destination)
            .unwrap_err()
            .to_string()
            .contains("dump failed")
    );
    assert!(!destination.exists());
}

#[test]
fn failed_container_creation_still_redacts_credentials() {
    let (_directory, docker) = docker(
        r#"
case "$1" in
    ps) ;;
    image) printf '%s\n' 'postgres:18' ;;
    create) printf '%s\n' "$POSTGRES_PASSWORD" >&2; exit 1 ;;
    *) exit 1 ;;
esac
"#,
    );
    let db = database();
    let error = docker.start(&db).unwrap_err().to_string();
    assert!(error.contains("[redacted]"));
    assert!(!error.contains(&db.password));
}
