//! Runs `lira check` in small git repositories. A program that stands in for
//! `gh` answers what the tests tell it to, so no test asks GitHub.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// What `gh` prints for the comments of a pull request: the body of each
/// comment, then this character.
const END: char = '\u{1e}';

const STAND_IN_FOR_GH: &str = r#"#!/bin/sh
# Stands in for `gh` in the tests of `lira check`.
here=$(dirname "$0")
echo "$*" >> "$here/calls"
if [ "$1 $2" = "auth status" ]; then
  [ -e "$here/no-login" ] && exit 1
  exit 0
fi
case "$2" in
  *pulls*) cat "$here/pulls" ;;
  *comments*) cat "$here/comments" ;;
  *) echo "the stand-in for gh does not know: $*" >&2; exit 1 ;;
esac
"#;

struct Repository {
    root: PathBuf,
    /// The folder of the stand-in for `gh`, and of what it prints.
    gh: PathBuf,
}

impl Repository {
    /// A repository with one commit on `main`, which has `lira.toml` and the
    /// script `check.sh`. The Lira file runs the script as its check.
    fn new(name: &str, scores: &str, check: &str) -> Repository {
        let top = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("check")
            .join(name);
        if top.exists() {
            fs::remove_dir_all(&top).unwrap();
        }
        let repository = Repository {
            root: top.join("repository"),
            gh: top.join("gh"),
        };
        fs::create_dir_all(&repository.root).unwrap();
        fs::create_dir_all(&repository.gh).unwrap();
        write_program(&repository.gh.join("gh"), STAND_IN_FOR_GH);
        repository.write("lira.toml", &format!("check = \"sh check.sh\"\n{scores}"));
        repository.write("check.sh", check);
        repository.git(&["init", "--quiet", "--initial-branch", "main"]);
        repository.commit("first");
        repository
    }

    fn write(&self, path: &str, content: &str) {
        fs::write(self.root.join(path), content).unwrap();
    }

    fn read(&self, path: &str) -> String {
        fs::read_to_string(self.root.join(path)).unwrap_or_default()
    }

    /// Runs git with no settings of the host, and returns what it prints.
    fn git(&self, args: &[&str]) -> String {
        let output = Command::new("git")
            .args([
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
            ])
            .args(args)
            .current_dir(&self.root)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .unwrap();
        assert!(output.status.success(), "git {args:?}: {}", stderr(&output));
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    /// Commits all files, and returns the commit.
    fn commit(&self, message: &str) -> String {
        self.git(&["add", "--all"]);
        self.git(&["commit", "--quiet", "--allow-empty", "--message", message]);
        self.git(&["rev-parse", "HEAD"])
    }

    /// The scores comment on the last merged pull request, as `gh` gives it.
    fn main_has(&self, pull: &str, comments: &[&str]) {
        let bodies: String = comments
            .iter()
            .map(|body| format!("{body}{END}\n"))
            .collect();
        fs::write(self.gh.join("pulls"), format!("{pull}\n")).unwrap();
        fs::write(self.gh.join("comments"), bodies).unwrap();
    }

    /// What Lira asked `gh`, one line for each call.
    fn calls(&self) -> String {
        fs::read_to_string(self.gh.join("calls")).unwrap_or_default()
    }

    fn lira(&self, args: &[&str]) -> Output {
        let path = std::env::var_os("PATH").unwrap();
        let mut folders = vec![self.gh.clone()];
        folders.extend(std::env::split_paths(&path));
        Command::new(env!("CARGO_BIN_EXE_lira"))
            .args(args)
            .current_dir(&self.root)
            .env("PATH", std::env::join_paths(folders).unwrap())
            .env("LIRA_SCORES", self.root.join("not-this-file"))
            .env("LIRA_BASE", "not-this-commit")
            .output()
            .unwrap()
    }

    fn check(&self) -> Output {
        self.lira(&["check"])
    }
}

/// Writes a program that a test can run at once. A child process writes it.
/// If this process had the file open for writing while another test started a
/// process, running the file could fail with `Text file busy`.
fn write_program(path: &Path, content: &str) {
    let mut child = Command::new("sh")
        .args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"])
        .arg(path)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    input.write_all(content.as_bytes()).unwrap();
    drop(input);
    assert!(child.wait().unwrap().success(), "{}", path.display());
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

const TWO_SCORES: &str = "\n[[score]]\nname = \"spec\"\nbetter = \"higher\"\n\n[[score]]\nname = \"size\"\nbetter = \"lower\"\n";

// spec(lira check): It runs the check command of the Lira file in the current folder, as the server runs it.
// spec(Check): Lira runs the check command in the worktree of the change.
#[test]
fn it_runs_the_check_command_in_the_current_folder() {
    let repository = Repository::new("runs", "", "pwd > where.txt\necho from the check\n");
    let output = repository.check();
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let folder = PathBuf::from(repository.read("where.txt").trim());
    assert_eq!(folder, repository.root.canonicalize().unwrap());
    assert_eq!(stdout(&output), "from the check\n");
}

// spec(lira check): Its exit status is 0 when the check passes, and 1 when it fails.
// spec(Check): The check passes when its exit status is 0.
// spec(Check): The check fails when its exit status is not 0.
#[test]
fn its_exit_status_is_0_when_the_check_passes_and_1_when_it_fails() {
    let repository = Repository::new("status", "", "exit 0\n");
    assert_eq!(repository.check().status.code(), Some(0));
    for status in [1, 2, 3, 127] {
        repository.write("check.sh", &format!("exit {status}\n"));
        assert_eq!(repository.check().status.code(), Some(1), "exit {status}");
    }
}

// spec(Check): Lira runs the check command with `sh -c`.
#[test]
fn the_check_command_is_a_line_for_the_shell() {
    let repository = Repository::new("shell", "", "");
    let line = "check = 'test -n \"$LIRA_BASE\" && echo \"a|b\" | tr a c > shell.txt; exit 0'\n";
    repository.write("lira.toml", line);
    assert_eq!(repository.check().status.code(), Some(0));
    assert_eq!(repository.read("shell.txt"), "c|b\n");
}

// spec(Check): Lira sets `LIRA_SCORES` to the path of an empty file before it runs the check.
#[test]
fn lira_scores_names_an_empty_file() {
    let script = "echo \"$LIRA_SCORES\" > path.txt\ntest -f \"$LIRA_SCORES\" && test ! -s \"$LIRA_SCORES\" && echo empty > seen.txt\n";
    let repository = Repository::new("scores-file", "", script);
    // The file that the caller names is not the file of this check.
    repository.write("not-this-file", "old = 1\n");
    assert_eq!(repository.check().status.code(), Some(0));
    assert_eq!(repository.read("seen.txt"), "empty\n");
    assert!(!repository.read("path.txt").contains("not-this-file"));
    assert_eq!(repository.read("not-this-file"), "old = 1\n");
}

// spec(Check): Lira sets `LIRA_BASE` to the commit of the main branch that the change is compared with.
#[test]
fn lira_base_is_the_commit_of_the_main_branch_that_the_change_starts_from() {
    let repository = Repository::new("base", "", "echo \"$LIRA_BASE\" > base.txt\n");
    let start = repository.git(&["rev-parse", "HEAD"]);
    // On the main branch, the change is compared with its own commit.
    assert_eq!(repository.check().status.code(), Some(0));
    assert_eq!(repository.read("base.txt").trim(), start);
    // The main branch moves on after the change starts.
    repository.git(&["switch", "--quiet", "--create", "change"]);
    repository.commit("the change");
    repository.git(&["switch", "--quiet", "main"]);
    let moved = repository.commit("the main branch moves");
    repository.git(&["switch", "--quiet", "change"]);
    assert_ne!(start, moved);
    assert_eq!(repository.check().status.code(), Some(0));
    assert_eq!(repository.read("base.txt").trim(), start);
}

// spec(lira check): It prints each score with its value, and its value on the main branch.
// spec(Scores): The check writes one line for each score to the file that `LIRA_SCORES` names.
// spec(Scores): A score line is `<name> = <number>`.
// spec(Scores): Lira reads the scores of the main branch from the comment on the last merged pull request.
#[test]
fn it_prints_each_score_with_its_value_and_its_value_on_the_main_branch() {
    let script = "echo 'size = 2.5' >> \"$LIRA_SCORES\"\necho 'spec = 14' >> \"$LIRA_SCORES\"\n";
    let repository = Repository::new("prints", TWO_SCORES, script);
    repository.main_has("7", &["Looks good.", "spec = 13\nsize = 2", "Thanks!"]);
    let output = repository.check();
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        "spec = 14, and 13 on the main branch\nsize = 2.5, and 2 on the main branch: worse\n"
    );
    // Lira asked for the merged pull requests of the main branch, then for
    // the comments of the last one.
    let calls = repository.calls();
    let calls: Vec<&str> = calls.lines().collect();
    assert_eq!(calls.len(), 3, "{calls:?}");
    assert_eq!(calls[0], "auth status");
    assert!(
        calls[1].contains("/pulls?state=closed&base=main&"),
        "{calls:?}"
    );
    assert!(calls[1].contains("merged_at"), "{calls:?}");
    assert!(calls[2].contains("/issues/7/comments"), "{calls:?}");
}

// spec(Scores): A score line with a name that the Lira file does not have is an error.
#[test]
fn a_score_with_a_name_that_the_lira_file_does_not_have_is_an_error() {
    let repository = Repository::new(
        "unknown",
        TWO_SCORES,
        "echo 'speed = 1' > \"$LIRA_SCORES\"\n",
    );
    repository.main_has("", &[]);
    let output = repository.check();
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains(
            "the check wrote the score `speed`, and lira.toml has no score with that name"
        ),
        "{}",
        stderr(&output)
    );
}

// spec(Scores): Two score lines with the same name are an error.
#[test]
fn two_score_lines_with_the_same_name_are_an_error() {
    let script = "echo 'spec = 1' >> \"$LIRA_SCORES\"\necho 'spec = 2' >> \"$LIRA_SCORES\"\n";
    let repository = Repository::new("twice", TWO_SCORES, script);
    repository.main_has("", &[]);
    let output = repository.check();
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("line 2: the score `spec` is there two times"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn a_line_that_is_not_a_score_line_is_an_error() {
    let repository = Repository::new(
        "not-a-line",
        TWO_SCORES,
        "echo 'spec: 1' > \"$LIRA_SCORES\"\n",
    );
    repository.main_has("", &[]);
    let output = repository.check();
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("a score line is `<name> = <number>`"),
        "{}",
        stderr(&output)
    );
}

// spec(Scores): A score that the check did not write has no value, and it is not compared.
#[test]
fn a_score_that_the_check_did_not_write_has_no_value() {
    let repository = Repository::new(
        "no-value",
        TWO_SCORES,
        "echo 'spec = 14' > \"$LIRA_SCORES\"\n",
    );
    repository.main_has("3", &["spec = 14\nsize = 2"]);
    let output = repository.check();
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        "spec = 14, and 14 on the main branch\nsize has no value, and 2 on the main branch\n"
    );
}

#[test]
fn with_no_merged_pull_request_no_score_has_a_value_on_the_main_branch() {
    let repository = Repository::new(
        "no-pull",
        TWO_SCORES,
        "echo 'spec = 1' > \"$LIRA_SCORES\"\n",
    );
    repository.main_has("", &[]);
    let output = repository.check();
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        "spec = 1, and no value on the main branch\nsize has no value, and no value on the main branch\n"
    );
    assert_eq!(repository.calls().lines().count(), 2);
}

#[test]
fn a_failing_check_still_prints_its_scores() {
    let repository = Repository::new(
        "fails",
        TWO_SCORES,
        "echo 'spec = 12' > \"$LIRA_SCORES\"\nexit 1\n",
    );
    repository.main_has("3", &["spec = 14"]);
    let output = repository.check();
    assert_eq!(output.status.code(), Some(1));
    assert!(stdout(&output).starts_with("spec = 12, and 14 on the main branch: worse\n"));
}

// spec(GitHub): Lira stops with an error that names `gh auth login` when `gh` has no login.
#[test]
fn with_no_login_of_gh_it_stops_before_the_check_runs() {
    let repository = Repository::new("no-login", TWO_SCORES, "echo ran > ran.txt\n");
    fs::write(repository.gh.join("no-login"), "").unwrap();
    let output = repository.check();
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("Run `gh auth login`"),
        "{}",
        stderr(&output)
    );
    assert_eq!(repository.read("ran.txt"), "");
    assert_eq!(repository.calls(), "auth status\n");
}

#[test]
fn a_repository_with_no_scores_asks_github_nothing() {
    let repository = Repository::new("no-scores", "", "exit 0\n");
    fs::write(repository.gh.join("no-login"), "").unwrap();
    let output = repository.check();
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stdout(&output), "");
    assert_eq!(repository.calls(), "");
}

#[test]
fn a_wrong_lira_file_is_a_failure_that_names_the_line() {
    let repository = Repository::new("wrong-file", "", "echo ran > ran.txt\n");
    repository.write("lira.toml", "check = \"sh check.sh\"\nchek = 1\n");
    let output = repository.check();
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).starts_with("lira: lira.toml:2: unknown key `chek`"),
        "{}",
        stderr(&output)
    );
    assert_eq!(repository.read("ran.txt"), "");
}

#[test]
fn a_folder_with_no_lira_file_is_a_failure() {
    let repository = Repository::new("no-file", "", "exit 0\n");
    fs::remove_file(repository.root.join("lira.toml")).unwrap();
    let output = repository.check();
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("there is no `lira.toml` in this folder"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn check_takes_no_arguments() {
    let repository = Repository::new("arguments", "", "echo ran > ran.txt\n");
    let output = repository.lira(&["check", "--fast"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("usage"));
    assert_eq!(repository.read("ran.txt"), "");
}
