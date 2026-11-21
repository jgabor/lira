//! Runs `cargo xtask check` in workspaces whose tests quote a spec.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SPEC: &str = "# A\n\nOne. Two.\n\n## B\n\nThree.\n\n- *Proposed:* Four.\n";
const QUOTES_ONE: &str = "// spec(A): One.\n#[test]\nfn one() {}\n";

/// A workspace with `crates/demo`, whose `src/lib.rs` is `lib`.
fn workspace(name: &str, lib: &str) -> PathBuf {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("check")
        .join(name);
    if root.exists() {
        fs::remove_dir_all(&root).unwrap();
    }
    let files = [
        (
            "Cargo.toml",
            "[workspace]\nmembers = [\"crates/*\"]\nresolver = \"3\"\n",
        ),
        ("SPEC.md", SPEC),
        (
            "crates/demo/Cargo.toml",
            "[package]\nname = \"demo\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
        ),
        ("crates/demo/src/lib.rs", lib),
    ];
    for (path, content) in files {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }
    let output = Command::new("cargo")
        .arg("generate-lockfile")
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(output.status.success());
    root
}

/// Runs the check in `root`. With `scores`, `LIRA_SCORES` names the file
/// `scores` in `root`, which is empty before the check runs.
fn check(root: &Path, scores: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_xtask"));
    command
        .arg("check")
        .current_dir(root)
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("LIRA_SCORES");
    if scores {
        fs::write(root.join("scores"), "").unwrap();
        command.env("LIRA_SCORES", root.join("scores"));
    }
    command.output().unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn scores(root: &Path) -> String {
    fs::read_to_string(root.join("scores")).unwrap()
}

#[test]
fn the_steps_run_in_order_and_the_score_is_one_line() {
    let root = workspace("passes", QUOTES_ONE);
    let output = check(&root, true);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        "pass format\npass lint\npass tests\npass quotes\nspec = 1\ncheck: pass\n"
    );
    assert_eq!(scores(&root), "spec = 1\n");
}

#[test]
fn without_lira_scores_no_file_is_written() {
    let root = workspace("unset", QUOTES_ONE);
    let output = check(&root, false);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(!root.join("scores").exists());
}

#[test]
fn a_promise_that_two_tests_quote_counts_once() {
    let lib = "// spec(A): One.\n#[test]\nfn a() {}\n\n// spec(A): One.\n// spec(B): Three.\n#[test]\nfn b() {}\n";
    let root = workspace("twice", lib);
    let output = check(&root, true);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(scores(&root), "spec = 2\n");
}

#[test]
fn a_quote_that_matches_no_sentence_fails_the_check() {
    let root = workspace("stale", "// spec(A): One!\n#[test]\nfn one() {}\n");
    let output = check(&root, true);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(stdout(&output).contains("pass tests\nfail quotes\n"));
    assert!(stdout(&output).ends_with("check: fail\n"));
    assert!(
        stderr(&output).contains("crates/demo/src/lib.rs:1: SPEC.md has no promise under `A`"),
        "{}",
        stderr(&output)
    );
    assert_eq!(scores(&root), "spec = 0\n");
}

#[test]
fn a_quote_of_a_proposed_sentence_fails_the_check() {
    let root = workspace("proposed", "// spec(B): Four.\n#[test]\nfn four() {}\n");
    let output = check(&root, true);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(stdout(&output).contains("fail quotes\n"));
}

#[test]
fn a_spec_comment_above_no_test_fails_the_check() {
    let root = workspace("orphan", "// spec(A): One.\npub fn one() {}\n");
    let output = check(&root, true);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(stdout(&output).contains("fail quotes\n"));
}

#[test]
fn a_failing_test_fails_the_check_and_scores_nothing() {
    let lib = "// spec(A): One.\n#[test]\nfn one() {\n    panic!();\n}\n";
    let root = workspace("failing", lib);
    let output = check(&root, true);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(stdout(&output).contains("fail tests\npass quotes\n"));
    assert_eq!(scores(&root), "spec = 0\n");
}

#[test]
fn an_ignored_test_scores_nothing() {
    let lib = "// spec(A): One.\n#[test]\n#[ignore]\nfn one() {}\n";
    let root = workspace("ignored", lib);
    let output = check(&root, true);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(scores(&root), "spec = 0\n");
}

#[test]
fn two_tests_with_one_name_can_not_quote() {
    let lib = "// spec(A): One.\n#[test]\nfn one() {}\n\n#[cfg(test)]\nmod other {\n    #[test]\n    fn one() {}\n}\n";
    let root = workspace("collision", lib);
    let output = check(&root, true);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(stderr(&output).contains("`one` names more than one test"));
}

#[test]
fn a_file_that_is_not_formatted_fails_the_format_test() {
    let root = workspace("unformatted", "pub fn  one(){}\n");
    let output = check(&root, false);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(stdout(&output).starts_with("fail format\npass lint\n"));
}

#[test]
fn a_lint_warning_fails_the_lint() {
    let root = workspace(
        "warning",
        "pub fn one() -> bool {\n    let two = true;\n    two\n}\n",
    );
    let output = check(&root, false);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(stdout(&output).starts_with("pass format\nfail lint\n"));
}

#[test]
fn check_takes_no_arguments() {
    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["check", "--fast"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("usage"));
}
