//! `urakata --version` prints the Cargo version.

use std::process::Command;

#[test]
fn version_flag() {
    let out = Command::new(env!("CARGO_BIN_EXE_urakata"))
        .arg("--version")
        .output()
        .expect("urakata must start");
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).expect("UTF-8 output");
    assert_eq!(
        text.trim_end(),
        format!("urakata {}", env!("CARGO_PKG_VERSION"))
    );
}
