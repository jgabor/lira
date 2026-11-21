//! Runs `lira --version`.

use std::process::{Command, Output};

fn lira(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lira"))
        .args(args)
        .output()
        .unwrap()
}

// spec(lira --version): It prints the version, and exits with status 0.
#[test]
fn version_prints_the_version_and_exits_with_status_0() {
    let output = lira(&["--version"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("lira {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn anything_else_is_bad_usage() {
    for args in [&[][..], &["--nope"][..], &["--version", "x"][..]] {
        let output = lira(args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("usage"));
    }
}
