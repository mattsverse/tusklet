use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command};

pub fn write_executable(path: &Path, script: &str) {
    // A concurrent process spawn can inherit an open writer until exec, causing
    // Linux to reject this script with ETXTBSY even after fs::write returns.
    // Create it in a child so no other test can inherit its writable descriptor.
    let status = Command::new("/bin/sh")
        .args(["-c", "printf '%s' \"$1\" > \"$2\"", "write-test-executable"])
        .arg(script)
        .arg(path)
        .status()
        .expect("write executable fixture");
    assert!(status.success(), "could not write executable fixture");
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}
