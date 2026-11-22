//! Runs `lira run` in a small repository with a remote in a folder beside
//! it. A shell script stands in for the harness, and another one for `gh`,
//! so no test asks an AI model or GitHub.

use lira::json::{self, Json};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// What each stand-in for the harness starts with. `next` reads one line
/// from Lira and keeps it. `say` writes one line to Lira. The stand-in keeps
/// the folder and the branch that it starts in. Then it answers
/// `initialize` and `session/new`, takes the AI model when Lira names one,
/// and reads the message for the agent.
const START: &str = r#"#!/bin/sh
here="$(dirname "$0")"
# Reads one line. `id` is the number of the newest request of Lira.
next() {
  IFS= read -r line || exit 0
  printf '%s\n' "$line" >> "$here/received"
  request=$(printf '%s\n' "$line" | sed -n 's/^{"jsonrpc":"2.0","id":\([0-9]*\),"method".*/\1/p')
  [ -n "$request" ] && id="$request"
}
say() { printf '%s\n' "$1"; }
# Tells Lira that the agent finished a tool call.
ended() { say '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"c-1","update":{"sessionUpdate":"tool_call_update","toolCallId":"t-1","status":"completed"}}}'; }
# Makes a commit with one new file: `commit <file> <subject>`.
commit() { echo "$1" > "$1.txt"; git add "$1.txt"; git commit --quiet --message "$2"; }
# Waits until `gh` was called with these words, for 10 seconds at most.
called() {
  tries=0
  until grep -q -- "$1" "$here/../bin/calls" 2>/dev/null; do
    tries=$((tries + 1)); [ "$tries" -gt 100 ] && return 1; sleep 0.1
  done
}
pwd > "$here/folder"
git branch --show-current > "$here/branch"
next; say '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1}}'
next; say '{"jsonrpc":"2.0","id":2,"result":{"sessionId":"c-1","configOptions":[{"id":"model","name":"Model","category":"model","type":"select","currentValue":"large-model","options":[]}]}}'
next
# The stand-in keeps the AI model that Lira chooses for the conversation.
case "$line" in
  *'"method":"session/set_config_option"'*'"configId":"model"'*)
    printf '%s\n' "$line" | sed 's/.*"value":"\([^"]*\)".*/\1/' > "$here/model"
    say '{"jsonrpc":"2.0","id":'"$id"',"result":{"configOptions":[]}}'
    next
    ;;
esac
printf '%s\n' "$line" > "$here/message"
"#;

/// The reply of the agent, and its end.
const REPLY: &str = r#"say '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"c-1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"I made the file."}}}}'
say '{"jsonrpc":"2.0","id":'"$id"',"result":{"stopReason":"end_turn"}}'
"#;

/// The work of an agent that makes one commit, and its reply.
const WORK: &str = r#"commit greeting 'Add a greeting'
say '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"c-1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"I made the file."}}}}'
say '{"jsonrpc":"2.0","id":'"$id"',"result":{"stopReason":"end_turn"}}'
"#;

/// Stands in for `gh`. It keeps each call, and the description that a call
/// gives. `gh pr create` prints the address of a pull request.
const GH: &str = r#"#!/bin/sh
here="$(dirname "$0")"
printf '%s\n' "$*" >> "$here/calls"
if [ "$1 $2" = "auth status" ]; then
  [ -e "$here/no-login" ] && exit 1
  exit 0
fi
description=""
before=""
for argument in "$@"; do
  [ "$before" = "--body-file" ] && description="$argument"
  before="$argument"
done
if [ "$1 $2" = "pr create" ]; then
  if [ -e "$here/no-create" ]; then
    echo "the stand-in for gh makes no pull request" >&2
    echo refused >> "$here/refused"
    exit 1
  fi
  if [ -e "$here/existing" ]; then
    echo "a pull request for this branch already exists" >&2
    exit 1
  fi
  # A slow answer of GitHub.
  [ -e "$here/slow-create" ] && sleep 1.5
  # An answer of GitHub that waits for the file `go`, for 10 seconds at most.
  if [ -e "$here/hold-create" ]; then
    echo started > "$here/create-started"
    tries=0
    until [ -e "$here/go" ]; do
      tries=$((tries + 1))
      if [ "$tries" -gt 200 ]; then echo "gave up" > "$here/gave-up"; exit 1; fi
      sleep 0.05
    done
  fi
  cp "$description" "$here/created"
  pwd > "$here/created-in"
  if [ -e "$here/lose-create" ]; then
    # GitHub opens the pull request, and its answer does not arrive.
    rm "$here/lose-create"
    echo "https://github.com/owner/name/pull/7" > "$here/existing"
    echo "the connection was lost" >&2
    exit 1
  fi
  echo "https://github.com/owner/name/pull/7"
  exit 0
fi
if [ "$1 $2" = "pr list" ]; then
  cat "$here/existing" 2>/dev/null
  exit 0
fi
if [ "$1 $2" = "pr edit" ]; then
  cp "$description" "$here/edited"
  # A slow answer of GitHub.
  [ -e "$here/slow-edit" ] && sleep 1
  exit 0
fi
echo "the stand-in for gh has no answer for: $1 $2" >&2
exit 1
"#;

/// The address that the stand-in for `gh` gives a new pull request.
const ADDRESS: &str = "https://github.com/owner/name/pull/7";

const CHECK: &str = "check = \"true\"\n";

const REQUEST: &str = "Add a file with a greeting";

struct Repository {
    /// The folder where a test writes the files of the main branch, and
    /// pushes them from.
    seed: PathBuf,
    /// The remote: the repository `owner/name`.
    remote: PathBuf,
    /// The repository on the host. `lira run` starts in it.
    root: PathBuf,
    /// The folder of the stand-in for the harness, and of what it keeps.
    harness: PathBuf,
    /// The folder of this test.
    top: PathBuf,
}

/// Runs git with no settings of the host. It returns what git prints, or
/// `None` when git fails.
fn git(folder: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
        ])
        .args(args)
        .current_dir(folder)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .unwrap();
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn git_ok(folder: &Path, args: &[&str]) -> String {
    git(folder, args).unwrap_or_else(|| panic!("git {args:?} failed in {}", folder.display()))
}

/// Makes a program from a script.
///
/// Another process writes the file. The tests run at the same time, and
/// each one starts processes. A process that starts gets, for a moment, the
/// files that this process has open. If this process had the script open
/// for writing in that moment, the system would refuse to run the script
/// until that other process is on its way.
fn program(path: &Path, script: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut writer = Command::new("sh")
        .args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"])
        .arg(path)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = writer.stdin.take().unwrap();
    input.write_all(script.as_bytes()).unwrap();
    drop(input);
    assert!(writer.wait().unwrap().success());
}

impl Repository {
    /// A repository whose main branch has one commit, with this Lira file.
    /// The stand-in for the harness does `work` when the message for the
    /// agent comes.
    fn new(test: &str, lira_file: &str, work: &str) -> Repository {
        let top = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("run")
            .join(test);
        if top.exists() {
            fs::remove_dir_all(&top).unwrap();
        }
        let repository = Repository {
            seed: top.join("seed"),
            remote: top.join("remote").join("owner").join("name.git"),
            root: top.join("repository"),
            harness: top.join("harness"),
            top: top.clone(),
        };
        fs::create_dir_all(&repository.seed).unwrap();
        fs::create_dir_all(repository.remote.parent().unwrap()).unwrap();
        fs::create_dir_all(top.join("home")).unwrap();
        git_ok(
            &repository.seed,
            &["init", "--quiet", "--initial-branch", "main"],
        );
        fs::write(repository.seed.join("README.md"), "first\n").unwrap();
        fs::write(repository.seed.join("lira.toml"), lira_file).unwrap();
        git_ok(&repository.seed, &["add", "--all"]);
        git_ok(
            &repository.seed,
            &["commit", "--quiet", "--message", "first"],
        );
        let remote = repository.remote.to_str().unwrap();
        git_ok(&top, &["clone", "--quiet", "--bare", "seed", remote]);
        git_ok(&top, &["clone", "--quiet", remote, "repository"]);
        program(&repository.harness.join("agent"), &format!("{START}{work}"));
        program(&top.join("bin").join("gh"), GH);
        repository
    }

    /// Puts a new commit on the main branch of the remote. The repository
    /// on the host does not have the commit yet.
    fn push(&self, path: &str, content: &str) -> String {
        fs::write(self.seed.join(path), content).unwrap();
        git_ok(&self.seed, &["add", "--all"]);
        git_ok(&self.seed, &["commit", "--quiet", "--message", "newer"]);
        git_ok(
            &self.seed,
            &["push", "--quiet", self.remote.to_str().unwrap(), "main"],
        );
        git_ok(&self.seed, &["rev-parse", "HEAD"])
    }

    /// `lira` in the repository. The data folder and the home folder are
    /// in the folder of the test, and the harness is the stand-in.
    fn lira(&self) -> Command {
        let path = std::env::var("PATH").unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_lira"));
        command
            .current_dir(&self.root)
            .env("XDG_DATA_HOME", self.top.join("xdg"))
            .env("HOME", self.top.join("home"))
            .env("LIRA_HARNESS", self.harness.join("agent"))
            .env("PATH", format!("{}:{path}", self.top.join("bin").display()))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.invalid");
        command
    }

    /// `lira run "<REQUEST>"`.
    fn run(&self) -> Output {
        self.lira().args(["run", REQUEST]).output().unwrap()
    }

    /// Where the worktrees of the repository are, in a data folder.
    fn worktrees(data: &Path) -> PathBuf {
        data.join("worktrees").join("owner").join("name")
    }

    /// The message that Lira wrote to the harness on this line. The first
    /// line is 1.
    fn received(&self, line: usize) -> Json {
        let text = fs::read_to_string(self.harness.join("received")).unwrap_or_default();
        let line = text
            .lines()
            .nth(line - 1)
            .unwrap_or_else(|| panic!("Lira wrote no line {line}:\n{text}"));
        json::read(line).unwrap()
    }

    /// What the stand-in for the harness kept in a file.
    fn kept(&self, file: &str) -> String {
        fs::read_to_string(self.harness.join(file))
            .unwrap_or_else(|e| panic!("the stand-in kept no `{file}`: {e}"))
            .trim()
            .to_string()
    }

    /// The text of the message for the agent.
    fn message(&self) -> String {
        let message = json::read(&self.kept("message")).unwrap();
        assert_eq!(
            message.get("method").and_then(Json::as_text),
            Some("session/prompt")
        );
        let params = message.get("params").unwrap();
        let parts = params.get("prompt").and_then(Json::as_list).unwrap();
        parts[0]
            .get("text")
            .and_then(Json::as_text)
            .unwrap()
            .to_string()
    }

    /// The calls of `gh` that start with these words, one on each line.
    fn calls(&self, start: &str) -> Vec<String> {
        let calls = fs::read_to_string(self.top.join("bin").join("calls")).unwrap_or_default();
        let calls = calls.lines().filter(|call| call.starts_with(start));
        calls.map(str::to_string).collect()
    }

    /// What the stand-in for `gh` kept in a file.
    fn gh(&self, file: &str) -> String {
        fs::read_to_string(self.top.join("bin").join(file))
            .unwrap_or_else(|e| panic!("the stand-in for gh kept no `{file}`: {e}"))
    }

    /// The commit of a branch of the remote, or `None` when the remote has
    /// no such branch.
    fn on_remote(&self, branch: &str) -> Option<String> {
        let full = format!("refs/heads/{branch}");
        git(&self.remote, &["rev-parse", "--verify", "--quiet", &full])
    }

    /// The branches of the repository that Lira made for a run.
    fn run_branches(&self) -> String {
        git_ok(&self.root, &["branch", "--list", "lira/run-*"])
    }

    /// If no worktree of a run is in this data folder.
    fn has_no_worktree(data: &Path) -> bool {
        let runs = Repository::worktrees(data).join("lira");
        !runs.exists() || fs::read_dir(runs).unwrap().next().is_none()
    }
}

fn errors(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn passed(output: &Output) {
    assert!(
        output.status.success(),
        "{:?}\n{}",
        output.status,
        errors(output)
    );
}

fn failed(output: &Output) -> String {
    assert_eq!(output.status.code(), Some(1), "{}", errors(output));
    errors(output)
}

// spec(lira run): `lira run "<request>"` runs an agent on the request, in a new worktree of the repository in the current folder.
// spec(lira run): The request needs no issue.
#[test]
fn lira_run_runs_an_agent_on_the_request_in_a_new_worktree() {
    let repository = Repository::new("request", CHECK, WORK);
    let newest = repository.push("NEW.md", "newer\n");
    passed(&repository.run());

    // The harness started in a folder, and the conversation is in the same
    // folder.
    let folder = PathBuf::from(repository.kept("folder"));
    let new = repository.received(2);
    assert_eq!(
        new.get("method").and_then(Json::as_text),
        Some("session/new")
    );
    let conversation = new.get("params").unwrap().get("cwd").unwrap();
    assert_eq!(
        Path::new(conversation.as_text().unwrap())
            .canonicalize()
            .unwrap(),
        folder.canonicalize().unwrap()
    );

    // The folder is a new worktree of the repository: git has it in its
    // list, and it is at the newest commit of the main branch.
    let worktrees = git_ok(&repository.root, &["worktree", "list", "--porcelain"]);
    let line = format!("worktree {}", folder.canonicalize().unwrap().display());
    assert!(worktrees.lines().any(|l| l == line), "{worktrees}");
    assert_ne!(
        folder.canonicalize().unwrap(),
        repository.root.canonicalize().unwrap()
    );
    // The agent made one commit on it.
    assert_eq!(git_ok(&folder, &["rev-parse", "HEAD~1"]), newest);

    // The agent got the request.
    assert!(repository.message().contains(REQUEST));

    // Lira asked GitHub for no issue, and made none.
    let calls = fs::read_to_string(repository.top.join("bin").join("calls")).unwrap_or_default();
    assert!(!calls.contains("issue"), "{calls}");
}

// spec(Server): The data folder is `$XDG_DATA_HOME/lira`.
#[test]
fn the_data_folder_is_in_xdg_data_home() {
    let repository = Repository::new("xdg", CHECK, WORK);
    passed(&repository.run());
    let folder = PathBuf::from(repository.kept("folder"));
    let data = repository.top.join("xdg").join("lira");
    assert!(
        folder.starts_with(Repository::worktrees(&data)),
        "{folder:?}"
    );
    assert!(Repository::has_no_worktree(
        &repository.top.join("home/.local/share/lira")
    ));
}

// spec(Server): When `XDG_DATA_HOME` is not set, the data folder is `~/.local/share/lira`.
#[test]
fn with_no_xdg_data_home_the_data_folder_is_in_the_home_folder() {
    let repository = Repository::new("home", CHECK, WORK);
    let output = repository
        .lira()
        .env_remove("XDG_DATA_HOME")
        .args(["run", REQUEST])
        .output()
        .unwrap();
    passed(&output);
    let folder = PathBuf::from(repository.kept("folder"));
    let data = repository.top.join("home/.local/share/lira");
    assert!(
        folder.starts_with(Repository::worktrees(&data)),
        "{folder:?}"
    );
}

// spec(Server): An `XDG_DATA_HOME` that is empty, or that is not an absolute path, counts as not set.
#[test]
fn an_xdg_data_home_that_is_empty_or_not_absolute_is_not_used() {
    for (test, value) in [("xdg-empty", ""), ("xdg-relative", "some/folder")] {
        let repository = Repository::new(test, CHECK, WORK);
        let output = repository
            .lira()
            .env("XDG_DATA_HOME", value)
            .args(["run", REQUEST])
            .output()
            .unwrap();
        passed(&output);
        let folder = PathBuf::from(repository.kept("folder"));
        let data = repository.top.join("home/.local/share/lira");
        assert!(
            folder.starts_with(Repository::worktrees(&data)),
            "{folder:?}"
        );
        assert!(!repository.root.join("some").exists());
    }
}

#[test]
fn with_no_folder_for_the_data_it_is_an_error() {
    let repository = Repository::new("no-data", CHECK, WORK);
    let output = repository
        .lira()
        .env_remove("XDG_DATA_HOME")
        .env_remove("HOME")
        .args(["run", REQUEST])
        .output()
        .unwrap();
    let error = failed(&output);
    assert!(error.contains("`XDG_DATA_HOME`"), "{error}");
    assert!(error.contains("`HOME`"), "{error}");
    assert!(!repository.harness.join("folder").exists());
    assert_eq!(repository.run_branches(), "");
}

// spec(Harnesses): The harness is the program that `LIRA_HARNESS` names.
#[test]
fn the_harness_is_the_program_that_lira_harness_names() {
    // The Lira file names a program too, and that one is not there.
    let lira_file = format!("{CHECK}[harness]\nprogram = \"not-there\"\n");
    let repository = Repository::new("named", &lira_file, WORK);
    let other = repository.top.join("other").join("my-harness");
    program(&other, &format!("{START}touch \"$here/started\"\n{WORK}"));
    let output = repository
        .lira()
        .env("LIRA_HARNESS", &other)
        .args(["run", REQUEST])
        .output()
        .unwrap();
    passed(&output);
    assert!(repository.top.join("other").join("started").exists());
    assert!(!repository.harness.join("folder").exists());
}

// spec(Format of the Lira file): | `harness.program` | Text | The program of the harness. |
// spec(Harnesses): When `LIRA_HARNESS` is not set, the harness is the program that `harness.program` in the Lira file names.
#[test]
fn with_no_lira_harness_the_harness_is_the_program_of_the_lira_file() {
    let lira_file = format!("{CHECK}[harness]\nprogram = \"file-harness\"\n");
    let repository = Repository::new("file-harness", &lira_file, WORK);
    // Two programs in a folder of `PATH`: the one that the Lira file names,
    // and the one that Lira starts when nothing names a program.
    let bin = repository.top.join("bin");
    for name in ["file-harness", "claude-agent-acp"] {
        program(
            &bin.join(name),
            &format!("{START}touch \"$here/started-{name}\"\n{WORK}"),
        );
    }
    for set in [false, true] {
        let _ = fs::remove_file(bin.join("started-file-harness"));
        let mut lira = repository.lira();
        lira.env_remove("LIRA_HARNESS");
        if set {
            // An empty value is the same as no value.
            lira.env("LIRA_HARNESS", "");
        }
        passed(&lira.args(["run", REQUEST]).output().unwrap());
        assert!(bin.join("started-file-harness").exists());
        assert!(!bin.join("started-claude-agent-acp").exists());
    }
}

// spec(Harnesses): When `LIRA_HARNESS` is not set and the Lira file has no `harness.program`, the harness is the program `claude-agent-acp`.
#[test]
fn with_no_lira_harness_and_no_key_the_harness_is_claude_agent_acp() {
    let repository = Repository::new("default", CHECK, WORK);
    // A program with that name, in a folder of `PATH`.
    let default = repository.top.join("bin").join("claude-agent-acp");
    program(&default, &format!("{START}touch \"$here/started\"\n{WORK}"));
    for set in [false, true] {
        let _ = fs::remove_file(repository.top.join("bin").join("started"));
        let mut lira = repository.lira();
        lira.env_remove("LIRA_HARNESS");
        if set {
            // An empty value is the same as no value.
            lira.env("LIRA_HARNESS", "");
        }
        passed(&lira.args(["run", REQUEST]).output().unwrap());
        assert!(repository.top.join("bin").join("started").exists());
    }
}

#[test]
fn a_path_in_lira_harness_starts_at_the_current_folder() {
    let repository = Repository::new("relative-harness", CHECK, WORK);
    // The harness starts in the worktree, and the path does not start
    // there.
    let output = repository
        .lira()
        .env("LIRA_HARNESS", "../harness/agent")
        .args(["run", REQUEST])
        .output()
        .unwrap();
    passed(&output);
    assert!(repository.harness.join("folder").exists());
}

#[test]
fn a_harness_that_is_not_there_is_an_error_and_leaves_no_worktree() {
    let repository = Repository::new("no-harness", CHECK, WORK);
    let nothing = repository.top.join("nothing-here");
    let output = repository
        .lira()
        .env("LIRA_HARNESS", &nothing)
        .args(["run", REQUEST])
        .output()
        .unwrap();
    let error = failed(&output);
    assert!(error.contains("nothing-here"), "{error}");
    assert!(error.contains("`LIRA_HARNESS`"), "{error}");
    assert!(error.contains("`harness.program`"), "{error}");
    assert!(Repository::has_no_worktree(
        &repository.top.join("xdg").join("lira")
    ));
    assert_eq!(repository.run_branches(), "");
    let worktrees = git_ok(&repository.root, &["worktree", "list", "--porcelain"]);
    assert_eq!(worktrees.matches("worktree ").count(), 1, "{worktrees}");
}

#[test]
fn a_harness_that_ends_before_the_agent_gets_the_message_leaves_no_worktree() {
    let repository = Repository::new("ends-early", CHECK, WORK);
    program(
        &repository.harness.join("agent"),
        "#!/bin/sh\necho 'I can not start' >&2\nexit 3\n",
    );
    let error = failed(&repository.run());
    assert!(error.contains("I can not start"), "{error}");
    assert!(Repository::has_no_worktree(
        &repository.top.join("xdg").join("lira")
    ));
    assert_eq!(repository.run_branches(), "");
}

/// If this is the name of the branch that a new worktree of a request
/// starts on.
fn is_for_a_run(branch: &str) -> bool {
    branch.strip_prefix("lira/run-").is_some_and(|end| {
        end.len() == 8
            && end
                .chars()
                .all(|c| c.is_ascii_digit() || c.is_ascii_lowercase())
    })
}

// spec(lira run): The new worktree starts on a branch `lira/run-<8 letters or digits>`.
#[test]
fn the_new_worktree_starts_on_a_branch_for_a_run() {
    let repository = Repository::new("branch", CHECK, WORK);
    passed(&repository.run());
    let first = repository.kept("branch");
    assert!(is_for_a_run(&first), "{first}");
    // The folder of the worktree has the name of the branch.
    let folder = PathBuf::from(repository.kept("folder"));
    let data = repository.top.join("xdg").join("lira");
    assert_eq!(folder, Repository::worktrees(&data).join(&first));

    // Each run has a branch and a worktree of its own.
    passed(&repository.run());
    let second = repository.kept("branch");
    assert!(is_for_a_run(&second), "{second}");
    assert_ne!(first, second);
    assert!(folder.exists());
}

// spec(lira run): The instructions tell the agent to give the branch a name that says what the work is, such as `fix/login-timeout`.
#[test]
fn the_instructions_tell_the_agent_to_name_the_branch() {
    let repository = Repository::new("instructions", CHECK, WORK);
    passed(&repository.run());
    let message = repository.message();
    let branch = repository.kept("branch");
    // The agent reads the name that the branch has now, how to change it,
    // and what a good name is.
    assert!(message.contains(&format!("`{branch}`")), "{message}");
    assert!(message.contains("git branch -m"), "{message}");
    assert!(message.contains("`fix/login-timeout`"), "{message}");
    // The instructions come first, and the request is the end.
    assert!(message.trim_end().ends_with(REQUEST), "{message}");
    assert!(!message.contains("{branch}"), "{message}");
}

#[test]
fn a_request_is_given_to_the_agent_as_it_is() {
    let repository = Repository::new("as-it-is", CHECK, WORK);
    let request = "Use {branch} and \"quotes\"\n\nand a second paragraph: é";
    let output = repository.lira().args(["run", request]).output().unwrap();
    passed(&output);
    assert!(repository.message().ends_with(request));
}

#[test]
fn lira_says_on_which_branch_the_work_is() {
    let work = "git branch -m feat/add-greeting\n";
    let repository = Repository::new("says-branch", CHECK, &format!("{work}{WORK}"));
    let output = repository.run();
    passed(&output);
    let wrote = errors(&output);
    let folder = repository.kept("folder");
    assert!(wrote.contains("`feat/add-greeting`"), "{wrote}");
    assert!(wrote.contains(&format!("`{folder}`")), "{wrote}");
    // What the agent said is there for the person to read.
    assert!(wrote.contains("I made the file."), "{wrote}");
    // The output is for the address of the pull request, and no other thing.
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("{ADDRESS}\n")
    );
    // The repository has the branch with the commit of the agent.
    assert_eq!(
        git_ok(
            &repository.root,
            &["log", "--format=%s", "-1", "feat/add-greeting"]
        ),
        "Add a greeting"
    );
    assert_eq!(repository.run_branches(), "");
}

// spec(Format of the Lira file): | `models.agent` | Text | The AI model for the agent. |
#[test]
fn the_agent_gets_the_ai_model_of_the_lira_file() {
    let lira_file = format!("{CHECK}[models]\nagent = \"small-model\"\n");
    let repository = Repository::new("model", &lira_file, WORK);
    passed(&repository.run());
    // The harness had another AI model for the conversation, and Lira
    // chose this one before the agent got the message.
    assert_eq!(repository.kept("model"), "small-model");
    assert!(repository.message().contains(REQUEST));

    // The AI model is a text. Another kind of value is an error, and no
    // agent starts.
    let lira_file = format!("{CHECK}[models]\nagent = 5\n");
    let repository = Repository::new("model-number", &lira_file, WORK);
    let error = failed(&repository.run());
    assert!(error.contains("`models.agent`"), "{error}");
    assert!(!repository.harness.join("folder").exists());
}

#[test]
fn with_no_ai_model_in_the_lira_file_lira_chooses_none() {
    let repository = Repository::new("no-model", CHECK, WORK);
    passed(&repository.run());
    assert!(!repository.harness.join("model").exists());
    let text = fs::read_to_string(repository.harness.join("received")).unwrap();
    assert!(!text.contains("session/set_config_option"), "{text}");
}

#[test]
fn a_harness_that_does_not_take_the_ai_model_is_an_error_and_leaves_no_worktree() {
    let lira_file = format!("{CHECK}[models]\nagent = \"small-model\"\n");
    let repository = Repository::new("model-refused", &lira_file, REPLY);
    // This harness has no option for the AI model.
    program(
        &repository.harness.join("agent"),
        r#"#!/bin/sh
IFS= read -r line; printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1}}'
IFS= read -r line; printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"sessionId":"c-1"}}'
IFS= read -r line && echo "$line" > "$(dirname "$0")/third"
"#,
    );
    let error = failed(&repository.run());
    assert!(error.contains("`small-model`"), "{error}");
    // The agent got no message with another AI model.
    assert!(!repository.harness.join("third").exists());
    assert!(Repository::has_no_worktree(
        &repository.top.join("xdg").join("lira")
    ));
    assert_eq!(repository.run_branches(), "");
}

#[test]
fn lira_says_yes_when_the_agent_asks_for_permission() {
    let work = r#"say '{"jsonrpc":"2.0","id":7,"method":"session/request_permission","params":{"sessionId":"c-1","toolCall":{"toolCallId":"t-1","title":"Write a.txt","kind":"edit"},"options":[{"optionId":"no","name":"No","kind":"reject_once"},{"optionId":"yes","name":"Yes","kind":"allow_once"}]}}'
next
"#;
    let repository = Repository::new("permission", CHECK, &format!("{work}{WORK}"));
    passed(&repository.run());
    let answer = repository.received(4);
    let expected =
        r#"{"jsonrpc":"2.0","id":7,"result":{"outcome":{"outcome":"selected","optionId":"yes"}}}"#;
    assert_eq!(answer, json::read(expected).unwrap());
}

#[test]
fn an_agent_that_stops_before_the_end_of_its_work_is_a_failure() {
    let work = r#"echo half > half.txt
say '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"c-1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"I started."}}}}'
say '{"jsonrpc":"2.0","id":3,"result":{"stopReason":"max_tokens"}}'
"#;
    let repository = Repository::new("stops", CHECK, work);
    let error = failed(&repository.run());
    assert!(error.contains("limit of tokens"), "{error}");
    assert!(error.contains("I started."), "{error}");
    // The worktree stays, with what the agent did, and Lira says where.
    let folder = repository.kept("folder");
    assert!(error.contains(&format!("`{folder}`")), "{error}");
    assert!(Path::new(&folder).join("half.txt").exists());
}

#[test]
fn a_harness_that_ends_while_the_agent_works_is_a_failure_and_the_worktree_stays() {
    let work = "echo half > half.txt\necho 'out of memory' >&2\nexit 9\n";
    let repository = Repository::new("ends-late", CHECK, work);
    let error = failed(&repository.run());
    assert!(error.contains("out of memory"), "{error}");
    let folder = repository.kept("folder");
    assert!(error.contains(&format!("`{folder}`")), "{error}");
    assert!(Path::new(&folder).join("half.txt").exists());
    assert!(is_for_a_run(
        &repository.run_branches().replace(['+', '*', ' '], "")
    ));
}

#[test]
fn lira_run_takes_one_request() {
    let repository = Repository::new("usage", CHECK, WORK);
    for args in [
        &["run"][..],
        &["run", "one", "two"],
        &["run", ""],
        &["run", " \n"],
        // Lira has no options for a run, so this is a mistake and no request.
        &["run", "--help"],
        &["run", "-h"],
        &["run", "-x\n"],
    ] {
        let output = repository.lira().args(args).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(errors(&output).contains("lira run \"<request>\""));
    }
    assert!(!repository.harness.join("folder").exists());
    assert_eq!(repository.run_branches(), "");
}

#[test]
fn a_request_can_start_with_a_dash() {
    let repository = Repository::new("dash", CHECK, WORK);
    let request = "- Add a greeting file\n- Keep it short";
    let output = repository.lira().args(["run", request]).output().unwrap();
    passed(&output);
    assert!(repository.message().ends_with(request));
}

#[test]
fn a_folder_that_is_no_repository_is_an_error() {
    let repository = Repository::new("no-repository", CHECK, WORK);
    let output = repository
        .lira()
        .current_dir(repository.top.join("home"))
        .env("GIT_CEILING_DIRECTORIES", &repository.top)
        .args(["run", REQUEST])
        .output()
        .unwrap();
    failed(&output);
    assert!(!repository.harness.join("folder").exists());
}

#[test]
fn lira_run_works_in_a_folder_inside_the_repository() {
    let repository = Repository::new("inside", CHECK, WORK);
    let inside = repository.root.join("some").join("folder");
    fs::create_dir_all(&inside).unwrap();
    let output = repository
        .lira()
        .current_dir(&inside)
        .args(["run", REQUEST])
        .output()
        .unwrap();
    passed(&output);
    let folder = PathBuf::from(repository.kept("folder"));
    let data = repository.top.join("xdg").join("lira");
    assert!(
        folder.starts_with(Repository::worktrees(&data)),
        "{folder:?}"
    );
}

// spec(lira run): Lira opens a draft pull request at the first commit of the agent.
// spec(Pull requests): Lira opens the pull request of an agent as a draft, at the first commit of the agent.
#[test]
fn lira_opens_a_draft_pull_request_at_the_first_commit_of_the_agent() {
    // The agent finishes a tool call with no commit, and then one with a
    // commit. It ends its reply only after the pull request is there.
    let work = r#"git branch -m feat/add-greeting
ended
sleep 0.3
cp "$here/../bin/calls" "$here/calls-before-the-commit" 2>/dev/null
commit greeting 'Add a greeting'
ended
called 'pr create' && touch "$here/opened-before-the-end"
"#;
    let repository = Repository::new("draft", CHECK, &format!("{work}{REPLY}"));
    passed(&repository.run());
    // No pull request before the first commit, and one before the reply
    // ended.
    let before = fs::read_to_string(repository.harness.join("calls-before-the-commit"));
    assert!(!before.unwrap_or_default().contains("pr create"));
    assert!(repository.harness.join("opened-before-the-end").exists());
    // It is one pull request, and it is a draft, from the branch of the
    // agent into the main branch.
    let created = repository.calls("pr create");
    assert_eq!(created.len(), 1, "{created:?}");
    let start = "pr create --draft --base main --head feat/add-greeting --title Add a greeting ";
    assert!(created[0].starts_with(start), "{created:?}");
    // The remote has the branch, with the commit of the agent.
    let folder = PathBuf::from(repository.kept("folder"));
    assert_eq!(
        repository.on_remote("feat/add-greeting"),
        Some(git_ok(&folder, &["rev-parse", "HEAD"]))
    );
    // `gh` ran in the worktree, so the pull request is in its repository.
    assert_eq!(
        Path::new(repository.gh("created-in").trim())
            .canonicalize()
            .unwrap(),
        folder.canonicalize().unwrap()
    );
}

// spec(Pull requests): The description of a draft pull request is the progress note.
// spec(Pull requests): The progress note has the headings `Done`, `Next` and `Decisions`.
#[test]
fn the_description_of_the_draft_is_the_progress_note() {
    let work = "commit greeting 'Add a greeting'\nended\ncalled 'pr create'\n";
    let repository = Repository::new("note", CHECK, &format!("{work}{REPLY}"));
    passed(&repository.run());
    let note = repository.gh("created");
    let lines: Vec<&str> = note.lines().collect();
    let place = |heading: &str| {
        lines
            .iter()
            .position(|line| *line == heading)
            .unwrap_or_else(|| panic!("the note has no `{heading}`:\n{note}"))
    };
    let (done, next, decisions) = (place("## Done"), place("## Next"), place("## Decisions"));
    assert!(done < next && next < decisions, "{note}");
    // These are all of its headings.
    assert_eq!(
        lines.iter().filter(|line| line.starts_with('#')).count(),
        3,
        "{note}"
    );
    // What is done is the commit of the agent.
    assert!(lines[done..next].contains(&"- Add a greeting"), "{note}");
    // It says what is next, and where the decisions of the agent come.
    assert!(
        lines[next..decisions].contains(&"The agent works on the request."),
        "{note}"
    );
    assert!(
        lines[decisions..].contains(&"The agent lists them when its work is complete."),
        "{note}"
    );
}

// spec(lira run): The title of the pull request is the subject of the first commit.
#[test]
fn the_title_of_the_pull_request_is_the_subject_of_the_first_commit() {
    let work = "commit one 'Add the first file'\ncommit two 'Add the second file'\n";
    let repository = Repository::new("title", CHECK, &format!("{work}{REPLY}"));
    passed(&repository.run());
    let created = repository.calls("pr create");
    assert_eq!(created.len(), 1, "{created:?}");
    assert!(
        created[0].contains("--title Add the first file --body-file"),
        "{created:?}"
    );
}

// spec(lira run): It prints the address of the pull request.
#[test]
fn lira_run_prints_the_address_of_the_pull_request() {
    let repository = Repository::new("address", CHECK, WORK);
    let output = repository.run();
    passed(&output);
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("{ADDRESS}\n")
    );
}

#[test]
fn lira_pushes_each_later_commit_and_brings_the_progress_note_up_to_date() {
    let work = r#"git branch -m feat/two
commit one 'Add the first file'
ended
called 'pr create'
commit two 'Add the second file'
git rev-parse HEAD > "$here/second"
ended
called 'pr edit'
git rev-parse --verify --quiet refs/heads/feat/two > /dev/null
(cd "$here/../remote/owner/name.git" && git rev-parse refs/heads/feat/two) > "$here/remote-at-the-edit"
"#;
    let repository = Repository::new("later", CHECK, &format!("{work}{REPLY}"));
    passed(&repository.run());
    // The remote had the second commit while the agent still worked.
    assert_eq!(
        repository.kept("remote-at-the-edit"),
        repository.kept("second")
    );
    assert_eq!(
        repository.on_remote("feat/two"),
        Some(repository.kept("second"))
    );
    let edited = repository.calls("pr edit");
    assert!(edited[0].starts_with(&format!("pr edit {ADDRESS} --body-file ")));
    let note = repository.gh("edited");
    assert!(
        note.contains("- Add the first file\n- Add the second file\n"),
        "{note}"
    );
    assert_eq!(repository.calls("pr create").len(), 1);
}

#[test]
fn a_commit_with_no_tool_call_is_in_the_pull_request_when_the_reply_ends() {
    // This harness says nothing about its tool calls.
    let repository = Repository::new("at-the-end", CHECK, WORK);
    let output = repository.run();
    passed(&output);
    let folder = PathBuf::from(repository.kept("folder"));
    let branch = repository.kept("branch");
    // The agent gave the branch no name, so it has the name that it
    // started with.
    assert_eq!(
        repository.on_remote(&branch),
        Some(git_ok(&folder, &["rev-parse", "HEAD"]))
    );
    let created = repository.calls("pr create");
    assert!(
        created[0].contains(&format!("--head {branch} ")),
        "{created:?}"
    );
}

// spec(lira run): When the agent makes no commit, Lira opens no pull request, and `lira run` fails.
#[test]
fn with_no_commit_there_is_no_pull_request() {
    let work = "echo half > half.txt\nended\n";
    let repository = Repository::new("no-commit", CHECK, &format!("{work}{REPLY}"));
    let output = repository.run();
    let error = failed(&output);
    assert!(error.contains("made no commit"), "{error}");
    assert_eq!(repository.calls("pr "), Vec::<String>::new());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
    // Nothing went to the remote.
    assert_eq!(
        git_ok(&repository.remote, &["for-each-ref", "--format=%(refname)"]),
        "refs/heads/main"
    );
    // The worktree stays, with what the agent did, and Lira says where.
    let folder = repository.kept("folder");
    assert!(error.contains(&format!("`{folder}`")), "{error}");
    assert!(Path::new(&folder).join("half.txt").exists());
    assert!(error.contains("I made the file."), "{error}");
}

#[test]
fn when_the_work_is_complete_the_progress_note_has_what_the_agent_wrote() {
    // The draft is there before the reply ends.
    let work = "commit greeting 'Add a greeting'\nended\ncalled 'pr create'\n";
    let repository = Repository::new("complete", CHECK, &format!("{work}{REPLY}"));
    passed(&repository.run());
    assert!(!repository.gh("created").contains("I made the file."));
    let note = repository.gh("edited");
    let decisions = note.split("## Decisions").nth(1).unwrap();
    assert!(decisions.contains("I made the file."), "{note}");
    assert!(note.contains("- Add a greeting\n"), "{note}");
}

/// A repository whose remote has a branch `feat/taken` of another person
/// already. It returns the commit of that branch too.
fn with_a_taken_branch(test: &str, work: &str) -> (Repository, Option<String>) {
    let repository = Repository::new(test, CHECK, work);
    git_ok(&repository.seed, &["branch", "feat/taken"]);
    let remote = repository.remote.to_str().unwrap();
    git_ok(&repository.seed, &["push", "--quiet", remote, "feat/taken"]);
    let theirs = repository.on_remote("feat/taken");
    assert!(theirs.is_some());
    (repository, theirs)
}

// spec(lira run): When the remote has a branch with the name that the agent chose, Lira tells the agent to give its branch another name.
#[test]
fn the_agent_renames_a_branch_that_the_remote_has_already() {
    // The agent gives its branch another name when a second message comes.
    let work = format!(
        "git branch -m feat/taken\n{WORK}next\nprintf '%s\\n' \"$line\" > \"$here/second\"\ngit branch -m feat/free\n{REPLY}"
    );
    let (repository, theirs) = with_a_taken_branch("taken", &work);
    let output = repository.run();
    passed(&output);
    // Lira told the agent which name the remote has, and how to change it.
    let second = repository.kept("second");
    assert!(second.contains("\"method\":\"session/prompt\""), "{second}");
    assert!(second.contains("`feat/taken`"), "{second}");
    assert!(second.contains("git branch -m"), "{second}");
    // The branch of the other person is as it was, and the work is on the
    // branch with the new name, and on no other branch.
    assert_eq!(repository.on_remote("feat/taken"), theirs);
    let folder = PathBuf::from(repository.kept("folder"));
    assert_eq!(
        repository.on_remote("feat/free"),
        Some(git_ok(&folder, &["rev-parse", "HEAD"]))
    );
    assert_eq!(repository.on_remote("feat/taken-2"), None);
    let created = repository.calls("pr create");
    assert_eq!(created.len(), 1, "{created:?}");
    assert!(created[0].contains("--head feat/free "), "{created:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), ADDRESS);
    // The progress note has what the agent wrote at the end of its work,
    // and not its answer to the second message.
    let note = repository.gh("created");
    assert!(note.contains("- Add a greeting\n"), "{note}");
}

#[test]
fn a_new_branch_is_another_name_for_a_taken_one() {
    // The agent does not give its branch another name. It makes a new
    // branch for its work, and leaves the one with the taken name.
    let work = format!(
        "git branch -m feat/taken\n{WORK}next\necho asked >> \"$here/asked\"\ngit checkout --quiet -b feat/free\n{REPLY}"
    );
    let (repository, theirs) = with_a_taken_branch("taken-new-branch", &work);
    passed(&repository.run());
    assert_eq!(repository.kept("asked").lines().count(), 1);
    assert_eq!(repository.on_remote("feat/taken"), theirs);
    let folder = PathBuf::from(repository.kept("folder"));
    assert_eq!(
        repository.on_remote("feat/free"),
        Some(git_ok(&folder, &["rev-parse", "HEAD"]))
    );
    let created = repository.calls("pr create");
    assert_eq!(created.len(), 1, "{created:?}");
    assert!(created[0].contains("--head feat/free "), "{created:?}");
}

#[test]
fn a_new_branch_with_no_work_is_not_another_name_for_a_taken_one() {
    // The new branch of the agent starts before its commit, so the work
    // stays on the branch with the taken name.
    let work = format!(
        "git branch -m feat/taken\n{WORK}while :; do\nnext\necho asked >> \"$here/asked\"\ngit checkout --quiet -B feat/free HEAD~1\n{REPLY}done\n"
    );
    let (repository, _) = with_a_taken_branch("taken-empty-branch", &work);
    let error = failed(&repository.run());
    assert!(error.contains("`feat/taken`"), "{error}");
    assert!(error.contains("3 messages"), "{error}");
    assert!(!error.contains("made no commit"), "{error}");
    assert_eq!(repository.kept("asked").lines().count(), 3);
    assert_eq!(repository.on_remote("feat/free"), None);
}

#[test]
fn an_agent_that_keeps_a_taken_name_is_a_failure_after_3_messages() {
    // The agent answers each message, and never changes the name.
    let work = format!(
        "git branch -m feat/taken\n{WORK}while :; do\nnext\necho asked >> \"$here/asked\"\n{REPLY}done\n"
    );
    let (repository, theirs) = with_a_taken_branch("never-renamed", &work);
    let error = failed(&repository.run());
    assert!(error.contains("`feat/taken`"), "{error}");
    assert!(error.contains("3 messages"), "{error}");
    assert_eq!(repository.kept("asked").lines().count(), 3);
    // Lira pushed nothing, and opened no pull request.
    assert_eq!(repository.on_remote("feat/taken"), theirs);
    assert_eq!(repository.calls("pr create"), Vec::<String>::new());
    let branches = git_ok(&repository.remote, &["branch", "--list"]);
    assert_eq!(branches.lines().count(), 2, "{branches}");
}

#[test]
fn a_commit_that_the_agent_changed_after_the_push_is_pushed_again() {
    let work = r#"git branch -m fix/amend
commit one 'Add a file'
ended
called 'pr create'
echo more >> one.txt
git commit --quiet --all --amend --message 'Add a better file'
ended
"#;
    let repository = Repository::new("amend", CHECK, &format!("{work}{REPLY}"));
    passed(&repository.run());
    let folder = PathBuf::from(repository.kept("folder"));
    assert_eq!(
        repository.on_remote("fix/amend"),
        Some(git_ok(&folder, &["rev-parse", "HEAD"]))
    );
    assert!(repository.gh("edited").contains("- Add a better file\n"));
    assert_eq!(repository.calls("pr create").len(), 1);
}

#[test]
fn a_new_name_for_the_branch_after_the_push_does_not_move_the_pull_request() {
    let work = r#"git branch -m fix/first-name
commit one 'Add a file'
ended
called 'pr create'
git branch -m fix/second-name
commit two 'Add one more file'
ended
"#;
    let repository = Repository::new("renamed", CHECK, &format!("{work}{REPLY}"));
    passed(&repository.run());
    let folder = PathBuf::from(repository.kept("folder"));
    assert_eq!(
        repository.on_remote("fix/first-name"),
        Some(git_ok(&folder, &["rev-parse", "HEAD"]))
    );
    assert_eq!(repository.on_remote("fix/second-name"), None);
    assert_eq!(repository.calls("pr create").len(), 1);
}

#[test]
fn lira_tries_again_when_github_did_not_open_the_pull_request() {
    // `gh` fails the first time, and works when the reply ends.
    let work = r#"commit one 'Add a file'
touch "$here/../bin/no-create"
ended
# The first try is over when `gh` said no.
until [ -e "$here/../bin/refused" ]; do sleep 0.05; done
rm "$here/../bin/no-create"
"#;
    let repository = Repository::new("again", CHECK, &format!("{work}{REPLY}"));
    let output = repository.run();
    passed(&output);
    assert_eq!(repository.calls("pr create").len(), 2);
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("{ADDRESS}\n")
    );
}

#[test]
fn a_pull_request_that_github_does_not_open_is_a_failure_that_says_why() {
    let repository = Repository::new("no-create", CHECK, WORK);
    fs::write(repository.top.join("bin").join("no-create"), "").unwrap();
    let output = repository.run();
    let error = failed(&output);
    assert!(
        error.contains("the stand-in for gh makes no pull request"),
        "{error}"
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
    // The work is on the remote, and Lira says on which branch, and where
    // the worktree is.
    let branch = repository.kept("branch");
    let folder = repository.kept("folder");
    assert!(repository.on_remote(&branch).is_some());
    assert!(error.contains(&format!("`{branch}`")), "{error}");
    assert!(error.contains(&format!("`{folder}`")), "{error}");
}

#[test]
fn a_push_that_the_remote_refuses_is_a_failure_that_shows_what_git_wrote() {
    let repository = Repository::new("no-push", CHECK, WORK);
    program(
        &repository.remote.join("hooks").join("pre-receive"),
        "#!/bin/sh\necho 'this remote takes no push' >&2\nexit 1\n",
    );
    let output = repository.run();
    let error = failed(&output);
    assert!(error.contains("this remote takes no push"), "{error}");
    assert_eq!(repository.calls("pr "), Vec::<String>::new());
    let folder = repository.kept("folder");
    assert!(error.contains(&format!("`{folder}`")), "{error}");
}

#[test]
fn with_no_login_lira_run_stops_before_it_makes_a_worktree() {
    let repository = Repository::new("no-login", CHECK, WORK);
    fs::write(repository.top.join("bin").join("no-login"), "").unwrap();
    let error = failed(&repository.run());
    assert!(error.contains("`gh auth login`"), "{error}");
    assert!(!repository.harness.join("folder").exists());
    assert!(Repository::has_no_worktree(
        &repository.top.join("xdg").join("lira")
    ));
    assert_eq!(repository.run_branches(), "");
}

#[test]
fn a_harness_that_ends_after_the_first_commit_leaves_the_draft_and_lira_names_it() {
    let work = "commit one 'Add a file'\nended\ncalled 'pr create'\nexit 9\n";
    let repository = Repository::new("ends-with-draft", CHECK, work);
    let output = repository.run();
    let error = failed(&output);
    assert!(error.contains(ADDRESS), "{error}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
}

/// If a process with this number runs. A process that ended, and that its
/// parent did not ask about yet, does not run.
fn runs(process: &str) -> bool {
    let state = Command::new("ps")
        .args(["-o", "stat=", "-p", process.trim()])
        .output()
        .unwrap();
    let state = String::from_utf8_lossy(&state.stdout);
    !state.trim().is_empty() && !state.trim().starts_with('Z')
}

/// Waits until `done` is true, for 10 seconds at most.
fn wait_for(what: &str, done: impl Fn() -> bool) {
    let start = std::time::Instant::now();
    while !done() {
        assert!(
            start.elapsed() < std::time::Duration::from_secs(10),
            "{what} did not happen in 10 seconds"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[test]
fn a_run_that_is_interrupted_leaves_no_harness_behind() {
    // This agent starts a process of its own, and never ends its reply.
    let work = r#"sleep 300 &
echo "$!" > "$here/child"
echo "$$" > "$here/harness"
while :; do sleep 1; done
"#;
    let repository = Repository::new("interrupted", CHECK, work);
    let mut lira = repository.lira().args(["run", REQUEST]).spawn().unwrap();
    // The file is there before the number is in it.
    let written = |file: &str| {
        fs::read_to_string(repository.harness.join(file)).is_ok_and(|text| text.ends_with('\n'))
    };
    wait_for("the start of the agent", || {
        written("harness") && written("child")
    });
    let (harness, child) = (repository.kept("harness"), repository.kept("child"));
    assert!(runs(&harness) && runs(&child));

    // The signal of Ctrl-C. Lira has no time to stop the harness.
    let lira_process = lira.id().to_string();
    assert!(
        Command::new("kill")
            .args(["-s", "INT", &lira_process])
            .status()
            .unwrap()
            .success()
    );
    assert!(!lira.wait().unwrap().success());
    wait_for("the end of the harness and of its child", || {
        !runs(&harness) && !runs(&child)
    });
}

#[test]
fn the_pull_request_keeps_the_work_of_the_agent_when_the_worktree_goes_to_another_branch() {
    // After the draft is there, the agent looks at another branch, with a
    // commit that is not its work for the request.
    let work = r#"git branch -m fix/mine
commit one 'Add a file'
git rev-parse HEAD > "$here/mine"
ended
called 'pr create'
git checkout --quiet -b theirs HEAD~1
commit other 'A change of another person'
ended
sleep 0.5
"#;
    let repository = Repository::new("other-branch", CHECK, &format!("{work}{REPLY}"));
    let output = repository.run();
    passed(&output);
    assert_eq!(
        repository.on_remote("fix/mine"),
        Some(repository.kept("mine"))
    );
    assert_eq!(repository.on_remote("theirs"), None);
    let calls = repository.calls("pr ");
    assert_eq!(repository.calls("pr create").len(), 1, "{calls:?}");
    for file in ["created", "edited"] {
        let note = fs::read_to_string(repository.top.join("bin").join(file)).unwrap_or_default();
        assert!(!note.contains("another person"), "{note}");
    }
}

#[test]
fn a_new_branch_that_the_agent_makes_for_its_work_is_the_branch_of_the_pull_request() {
    // This agent does not give the branch a new name. It makes a new one.
    let work = "git checkout --quiet -b feat/new-branch\n";
    let repository = Repository::new("new-branch", CHECK, &format!("{work}{WORK}"));
    passed(&repository.run());
    let folder = PathBuf::from(repository.kept("folder"));
    assert_eq!(
        repository.on_remote("feat/new-branch"),
        Some(git_ok(&folder, &["rev-parse", "HEAD"]))
    );
    let created = repository.calls("pr create");
    assert!(
        created[0].contains("--head feat/new-branch "),
        "{created:?}"
    );
    assert_eq!(repository.on_remote(&repository.kept("branch")), None);
}

#[test]
fn a_branch_that_was_there_before_the_run_is_not_the_work_of_the_agent() {
    // The repository has a branch of the person, with a commit. The agent
    // goes to it, and makes no commit of its own.
    let work = "git checkout --quiet mine\nended\n";
    let repository = Repository::new("old-branch", CHECK, &format!("{work}{REPLY}"));
    git_ok(&repository.root, &["checkout", "--quiet", "-b", "mine"]);
    fs::write(repository.root.join("mine.txt"), "mine\n").unwrap();
    git_ok(&repository.root, &["add", "--all"]);
    git_ok(
        &repository.root,
        &["commit", "--quiet", "--message", "My own work"],
    );
    git_ok(&repository.root, &["checkout", "--quiet", "--detach"]);
    // A tag with the same name changes how git prints the short name of
    // the branch.
    git_ok(&repository.root, &["tag", "mine", "HEAD~1"]);
    let error = failed(&repository.run());
    assert!(error.contains("made no commit"), "{error}");
    assert_eq!(repository.on_remote("mine"), None);
    assert_eq!(repository.calls("pr "), Vec::<String>::new());
}

#[test]
fn an_agent_that_takes_its_commits_back_after_the_draft_is_a_failure() {
    let work = r#"git branch -m fix/taken-back
commit one 'Add a file'
git rev-parse HEAD > "$here/one"
ended
called 'pr create'
git reset --quiet --hard HEAD~1
ended
"#;
    let repository = Repository::new("taken-back", CHECK, &format!("{work}{REPLY}"));
    let output = repository.run();
    let error = failed(&output);
    assert!(error.contains("took its commits back"), "{error}");
    assert!(error.contains(ADDRESS), "{error}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
    // Lira removes no work from the remote.
    assert_eq!(
        repository.on_remote("fix/taken-back"),
        Some(repository.kept("one"))
    );
}

impl Repository {
    /// Puts a program before `git` that loses the answer to one push: the
    /// remote takes the push, and git says that it failed. It does so
    /// while the file `lose-push` is there, one time.
    fn git_that_loses_a_push(&self) {
        let real = Command::new("sh")
            .args(["-c", "command -v git"])
            .output()
            .unwrap();
        let real = String::from_utf8_lossy(&real.stdout).trim().to_string();
        let script = format!(
            r#"#!/bin/sh
here="$(dirname "$0")"
if [ "$1" = push ] && [ -e "$here/lose-push" ]; then
  # With the file `ask-too`, the next question to the remote fails too.
  [ -e "$here/ask-too" ] && touch "$here/lose-ask"
  "{real}" "$@"
  echo "the connection was lost" >&2
  rm "$here/lose-push"
  exit 1
fi
if [ "$1" = ls-remote ] && [ -e "$here/lose-ask" ]; then
  echo "the connection was lost" >&2
  rm "$here/lose-ask"
  exit 1
fi
exec "{real}" "$@"
"#
        );
        program(&self.top.join("bin").join("git"), &script);
    }
}

#[test]
fn a_push_whose_answer_got_lost_does_not_stop_the_next_push() {
    let work = r#"git branch -m fix/lost
commit one 'Add a file'
ended
called 'pr create'
touch "$here/../bin/lose-push"
commit two 'Add a second file'
ended
until [ ! -e "$here/../bin/lose-push" ]; do sleep 0.05; done
commit three 'Add a third file'
ended
"#;
    let repository = Repository::new("lost-push", CHECK, &format!("{work}{REPLY}"));
    repository.git_that_loses_a_push();
    let output = repository.run();
    passed(&output);
    let folder = PathBuf::from(repository.kept("folder"));
    assert_eq!(
        repository.on_remote("fix/lost"),
        Some(git_ok(&folder, &["rev-parse", "HEAD"]))
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("{ADDRESS}\n")
    );
}

#[test]
fn a_first_push_whose_answer_got_lost_makes_no_second_branch() {
    let work = r#"git branch -m fix/lost-first
touch "$here/../bin/lose-push"
commit one 'Add a file'
ended
until [ ! -e "$here/../bin/lose-push" ]; do sleep 0.05; done
sleep 0.3
"#;
    let repository = Repository::new("lost-first-push", CHECK, &format!("{work}{REPLY}"));
    repository.git_that_loses_a_push();
    passed(&repository.run());
    let folder = PathBuf::from(repository.kept("folder"));
    assert_eq!(
        repository.on_remote("fix/lost-first"),
        Some(git_ok(&folder, &["rev-parse", "HEAD"]))
    );
    assert_eq!(repository.on_remote("fix/lost-first-2"), None);
    let created = repository.calls("pr create");
    assert_eq!(created.len(), 1, "{created:?}");
    assert!(created[0].contains("--head fix/lost-first "), "{created:?}");
}

#[test]
fn a_pull_request_whose_answer_got_lost_is_found_again() {
    let repository = Repository::new("lost-create", CHECK, WORK);
    fs::write(repository.top.join("bin").join("lose-create"), "").unwrap();
    let output = repository.run();
    passed(&output);
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("{ADDRESS}\n")
    );
    // Lira asked GitHub for the pull request of the branch, and opened no
    // second one.
    assert_eq!(repository.calls("pr create").len(), 1);
    let branch = repository.kept("branch");
    let listed = repository.calls("pr list");
    assert!(
        listed[0].contains(&format!("--head {branch} ")),
        "{listed:?}"
    );
    // A pull request from a copy of the repository on GitHub can have a
    // branch with the same name. Lira asks for one of this repository.
    assert!(listed[0].contains("isCrossRepository"), "{listed:?}");
    assert!(
        listed[0].contains("select(.isCrossRepository | not)"),
        "{listed:?}"
    );
    // Lira does not know which description the pull request got, so it
    // writes the description again.
    let edited = repository.calls("pr edit");
    assert_eq!(edited.len(), 1, "{edited:?}");
    assert!(repository.gh("edited").contains("- Add a greeting\n"));
}

#[test]
fn the_agent_does_not_wait_while_lira_opens_the_pull_request() {
    // GitHub does not answer until the agent has its permission. The agent
    // asks for the permission after Lira started to open the pull request.
    let work = r#"commit one 'Add a file'
ended
until [ -e "$here/../bin/create-started" ]; do sleep 0.05; done
say '{"jsonrpc":"2.0","id":9,"method":"session/request_permission","params":{"sessionId":"c-1","toolCall":{"toolCallId":"t-2","title":"Write b.txt","kind":"edit"},"options":[{"optionId":"yes","name":"Yes","kind":"allow_once"}]}}'
next
touch "$here/../bin/go"
"#;
    let repository = Repository::new("slow-github", CHECK, &format!("{work}{REPLY}"));
    fs::write(repository.top.join("bin").join("hold-create"), "").unwrap();
    let output = repository.run();
    passed(&output);
    // GitHub got its sign from the agent, and did not give up before.
    assert!(!repository.top.join("bin").join("gave-up").exists());
    assert_eq!(repository.calls("pr create").len(), 1);
}

#[test]
fn a_harness_that_lira_can_not_guard_does_not_start() {
    let repository = Repository::new("no-guard", CHECK, REPLY);
    // This harness needs no other program, and it answers nothing.
    program(
        &repository.harness.join("agent"),
        "#!/bin/sh\necho \"$$\" > \"${0%/*}/process\"\nexec /bin/sleep 20\n",
    );
    // A `PATH` with git and `gh`, and with no `sh` for the guard.
    let only = repository.top.join("only");
    fs::create_dir_all(&only).unwrap();
    let git = Command::new("sh")
        .args(["-c", "command -v git"])
        .output()
        .unwrap();
    let git = String::from_utf8_lossy(&git.stdout).trim().to_string();
    std::os::unix::fs::symlink(git, only.join("git")).unwrap();
    let path = format!(
        "{}:{}",
        only.display(),
        repository.top.join("bin").display()
    );
    let output = repository
        .lira()
        .env("PATH", path)
        .args(["run", REQUEST])
        .output()
        .unwrap();
    let error = failed(&output);
    assert!(error.contains("`sh`"), "{error}");
    // The harness does not stay, and the worktree is gone.
    if let Ok(process) = fs::read_to_string(repository.harness.join("process")) {
        wait_for("the end of the harness", || !runs(&process));
    }
    assert!(Repository::has_no_worktree(
        &repository.top.join("xdg").join("lira")
    ));
}

#[test]
fn a_commit_that_comes_after_the_reply_is_in_the_pull_request() {
    // Lira needs the harness after the reply, to ask for another name of
    // the branch. So a child of the harness can still make a commit then.
    // This one makes it while Lira writes the progress note at the end.
    let work = r#"git branch -m fix/late-commit
commit one 'Add a file'
ended
called 'pr create'
(until [ -e "$here/../bin/edited" ]; do sleep 0.05; done; commit late 'A late commit'; sleep 30) > /dev/null 2>&1 &
echo "$!" > "$here/child"
"#;
    let repository = Repository::new("late-child", CHECK, &format!("{work}{REPLY}"));
    fs::write(repository.top.join("bin").join("slow-edit"), "").unwrap();
    passed(&repository.run());
    // Lira looked one more time after it stopped the harness.
    let folder = PathBuf::from(repository.kept("folder"));
    assert_eq!(
        git_ok(&folder, &["log", "-1", "--format=%s"]),
        "A late commit"
    );
    assert_eq!(
        repository.on_remote("fix/late-commit"),
        Some(git_ok(&folder, &["rev-parse", "HEAD"]))
    );
    let note = repository.gh("edited");
    assert!(note.contains("- A late commit\n"), "{note}");
    // Lira stopped the harness and its child, so nothing changes the work
    // after that look.
    assert!(!runs(&repository.kept("child")));
}

#[test]
fn a_first_push_that_lira_could_not_ask_about_makes_no_second_branch() {
    // The remote takes the first push. Its answer gets lost, and so does
    // the answer when Lira asks the remote for the branch. Then the agent
    // gives its branch another name.
    let work = r#"git branch -m fix/first-name
touch "$here/../bin/lose-push"
commit one 'Add a file'
ended
until [ ! -e "$here/../bin/lose-push" ] && [ ! -e "$here/../bin/lose-ask" ]; do sleep 0.05; done
sleep 0.3
git branch -m fix/second-name
commit two 'Add a second file'
ended
"#;
    let repository = Repository::new("lost-push-and-ask", CHECK, &format!("{work}{REPLY}"));
    repository.git_that_loses_a_push();
    fs::write(repository.top.join("bin").join("ask-too"), "").unwrap();
    passed(&repository.run());
    let folder = PathBuf::from(repository.kept("folder"));
    assert_eq!(
        repository.on_remote("fix/first-name"),
        Some(git_ok(&folder, &["rev-parse", "HEAD"]))
    );
    assert_eq!(repository.on_remote("fix/second-name"), None);
    let created = repository.calls("pr create");
    assert_eq!(created.len(), 1, "{created:?}");
    assert!(created[0].contains("--head fix/first-name "), "{created:?}");
}

#[test]
fn commits_that_the_agent_takes_from_the_main_branch_are_not_its_work() {
    // While the agent works, the main branch of the remote gets a commit
    // of another person. The agent puts its work on top of it.
    let work = r#"git branch -m fix/on-newer-main
git clone --quiet "$(git remote get-url origin)" "$here/other"
(cd "$here/other" && commit theirs 'A commit of another person' && git push --quiet origin main)
git pull --quiet --rebase origin main
commit mine 'Add my file'
"#;
    let repository = Repository::new("newer-main", CHECK, &format!("{work}{REPLY}"));
    passed(&repository.run());
    let created = repository.calls("pr create");
    assert!(
        created[0].contains("--title Add my file --body-file"),
        "{created:?}"
    );
    let note = repository.gh("created");
    assert!(note.contains("- Add my file\n"), "{note}");
    assert!(!note.contains("another person"), "{note}");
}
