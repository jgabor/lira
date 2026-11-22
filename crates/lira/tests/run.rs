//! Runs `lira run` in a small repository with a remote in a folder beside
//! it. A shell script stands in for the harness, and another one for `gh`,
//! so no test asks an AI model or GitHub.

use lira::json::{self, Json};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// What each stand-in for the harness starts with. `next` reads one line
/// from Lira and keeps it. `say` writes one line to Lira. The stand-in
/// answers `initialize` and `session/new`, and reads the first message.
///
/// Lira starts the same program for the agent and for each Reviewer. The
/// first message says which one it is. For a Reviewer, the stand-in runs
/// the script `reviewer.sh` when a test wrote one, and says that the change
/// is good when no test did. For the agent, it keeps the folder and the
/// branch that it starts in, and then runs what follows this text.
const START: &str = r#"#!/bin/sh
here="$(dirname "$0")"
# Each start has a number. Start 1 is the agent.
start=$(( $(cat "$here/starts" 2>/dev/null || echo 0) + 1 ))
echo "$start" > "$here/starts"
received="$here/received"
[ "$start" -gt 1 ] && received="$here/received-$start"
# Reads one line. `id` is the number of the newest request of Lira.
next() {
  IFS= read -r line || exit 0
  printf '%s\n' "$line" >> "$received"
  request=$(printf '%s\n' "$line" | sed -n 's/^{"jsonrpc":"2.0","id":\([0-9]*\),"method".*/\1/p')
  [ -n "$request" ] && id="$request"
}
say() { printf '%s\n' "$1"; }
# Says a text as the reply to the newest message, and ends the reply.
reply() {
  say '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"c-1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"'"$1"'"}}}}'
  say '{"jsonrpc":"2.0","id":'"$id"',"result":{"stopReason":"end_turn"}}'
}
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
next; say '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1}}'
next; say '{"jsonrpc":"2.0","id":2,"result":{"sessionId":"c-1"}}'
next
case "$line" in
  *"You are a Reviewer"*)
    pwd > "$here/review-folder"
    if [ -e "$here/reviewer.sh" ]; then
      . "$here/reviewer.sh"
    else
      reply 'The change is good.\n\nReview: pass'
    fi
    exit 0
    ;;
esac
pwd > "$here/folder"
git branch --show-current > "$here/branch"
"#;

/// The reply of the agent, and its end.
const REPLY: &str = r#"say '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"c-1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"I made the file."}}}}'
say '{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}'
"#;

/// The work of an agent that makes one commit, and its reply.
const WORK: &str = r#"commit greeting 'Add a greeting'
say '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"c-1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"I made the file."}}}}'
say '{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}'
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
    exit 1
  fi
  cp "$description" "$here/created"
  pwd > "$here/created-in"
  echo "https://github.com/owner/name/pull/7"
  exit 0
fi
if [ "$1 $2" = "pr edit" ]; then
  cp "$description" "$here/edited"
  exit 0
fi
if [ "$1 $2" = "pr ready" ]; then
  [ -n "$ORDER" ] && echo ready >> "$ORDER"
  cp "$here/edited" "$here/description-when-ready" 2>/dev/null || cp "$here/created" "$here/description-when-ready"
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

fn program(path: &Path, script: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, script).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

impl Repository {
    /// A repository whose main branch has one commit, with this Lira file.
    /// The stand-in for the harness does `work` when the message for the
    /// agent comes.
    fn new(test: &str, lira_file: &str, work: &str) -> Repository {
        Repository::with(test, lira_file, "", work)
    }

    /// A repository whose check command runs the script `check`. The main
    /// branch has the script.
    fn checked(test: &str, check: &str, work: &str) -> Repository {
        Repository::with(test, "check = \"sh check.sh\"\n", check, work)
    }

    fn with(test: &str, lira_file: &str, check: &str, work: &str) -> Repository {
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
        if !check.is_empty() {
            fs::write(repository.seed.join("check.sh"), check).unwrap();
        }
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
        let message = self.received(3);
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

    /// Gives the stand-in the script that it runs as a Reviewer.
    fn reviewer(&self, script: &str) {
        fs::write(self.harness.join("reviewer.sh"), script).unwrap();
    }

    /// How many times Lira started the harness.
    fn starts(&self) -> usize {
        self.kept("starts").parse().unwrap()
    }

    /// The text of the first message of a start of the harness. Start 1 is
    /// the agent, and each later start is a Reviewer.
    fn first_message(&self, start: usize) -> String {
        let file = match start {
            1 => "received".to_string(),
            start => format!("received-{start}"),
        };
        let text = fs::read_to_string(self.harness.join(&file))
            .unwrap_or_else(|e| panic!("the stand-in kept no `{file}`: {e}"));
        let message = json::read(text.lines().nth(2).unwrap()).unwrap();
        text_of(&message)
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

/// The text of a message that Lira wrote for an agent.
fn text_of(message: &Json) -> String {
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

/// The text of the message that the stand-in kept in a file, as one line.
fn kept_message(repository: &Repository, file: &str) -> String {
    text_of(&json::read(&repository.kept(file)).unwrap())
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
    let repository = Repository::new("named", CHECK, WORK);
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

// spec(Harnesses): When `LIRA_HARNESS` is not set, the harness is the program `claude-agent-acp`.
#[test]
fn with_no_lira_harness_the_harness_is_claude_agent_acp() {
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
    let new = repository.received(2);
    let model = new
        .get("params")
        .and_then(|params| params.get("_meta"))
        .and_then(|meta| meta.get("claudeCode"))
        .and_then(|claude| claude.get("options"))
        .and_then(|options| options.get("model"))
        .and_then(Json::as_text);
    assert_eq!(model, Some("small-model"), "{new}");
}

#[test]
fn with_no_ai_model_in_the_lira_file_lira_names_none() {
    let repository = Repository::new("no-model", CHECK, WORK);
    passed(&repository.run());
    let new = repository.received(2);
    assert_eq!(new.get("params").unwrap().get("_meta"), None, "{new}");
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
    ] {
        let output = repository.lira().args(args).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(errors(&output).contains("lira run \"<request>\""));
    }
    assert!(!repository.harness.join("folder").exists());
    assert_eq!(repository.run_branches(), "");
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

#[test]
fn a_branch_that_the_remote_has_already_gets_another_name_there() {
    let work = "git branch -m feat/taken\n";
    let repository = Repository::new("taken", CHECK, &format!("{work}{WORK}"));
    // Another person pushed a branch with this name.
    git_ok(&repository.seed, &["branch", "feat/taken"]);
    let remote = repository.remote.to_str().unwrap();
    git_ok(&repository.seed, &["push", "--quiet", remote, "feat/taken"]);
    let theirs = repository.on_remote("feat/taken");

    let output = repository.run();
    passed(&output);
    assert_eq!(repository.on_remote("feat/taken"), theirs);
    let folder = PathBuf::from(repository.kept("folder"));
    assert_eq!(
        repository.on_remote("feat/taken-2"),
        Some(git_ok(&folder, &["rev-parse", "HEAD"]))
    );
    let created = repository.calls("pr create");
    assert!(created[0].contains("--head feat/taken-2 "), "{created:?}");
    assert!(
        errors(&output).contains("`feat/taken-2`"),
        "{}",
        errors(&output)
    );
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
called 'pr create'
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

/// A check that passes when the change has the file `fixed.txt`. It keeps
/// each run in the file that `ORDER` names, and the folder that it ran in.
const NEEDS_FIXED: &str = r#"echo check >> "$ORDER"
pwd > "$ORDER-folder"
echo "the check looks for fixed.txt"
if [ ! -e fixed.txt ]; then
  echo "and fixed.txt is not there" >&2
  exit 1
fi
"#;

/// An agent that makes a commit, and then repairs the change when Lira
/// sends it a second message. It keeps the second message.
const WORK_THEN_REPAIR: &str = r#"commit greeting 'Add a greeting'
reply 'I made the file.'
next
printf '%s\n' "$line" > "$here/second-message"
commit fixed 'Add the fixed file'
reply 'I repaired it.'
"#;

impl Repository {
    /// `lira run`, with a file that the check and the Reviewer write their
    /// order in.
    fn run_in_order(&self) -> Output {
        self.lira()
            .env("ORDER", self.top.join("order"))
            .args(["run", REQUEST])
            .output()
            .unwrap()
    }

    fn order(&self) -> String {
        fs::read_to_string(self.top.join("order"))
            .unwrap_or_default()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }
}

// spec(Steps): When the check fails, Lira gives the output of the check to the agent.
#[test]
fn when_the_check_fails_lira_gives_its_output_to_the_agent() {
    let repository = Repository::checked("check-fails", NEEDS_FIXED, WORK_THEN_REPAIR);
    let output = repository.run_in_order();
    passed(&output);
    // The agent got what the check wrote, to its output and to its error
    // output.
    let second = kept_message(&repository, "second-message");
    assert!(second.contains("the check looks for fixed.txt"), "{second}");
    assert!(second.contains("and fixed.txt is not there"), "{second}");
    // It is all that the check wrote, and the message says so.
    assert!(second.contains("This is what the check wrote:"), "{second}");
    assert!(!second.contains("left out"), "{second}");
    // The check ran in the worktree of the change, before and after the
    // repair.
    assert_eq!(repository.order(), "check check ready");
    assert_eq!(
        fs::read_to_string(repository.top.join("order-folder"))
            .unwrap()
            .trim(),
        repository.kept("folder")
    );
    // The repair is in the pull request.
    let folder = PathBuf::from(repository.kept("folder"));
    let branch = repository.kept("branch");
    assert_eq!(
        repository.on_remote(&branch),
        Some(git_ok(&folder, &["rev-parse", "HEAD"]))
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("{ADDRESS}\n")
    );
}

#[test]
fn a_check_that_fails_three_times_ends_the_run_with_a_failure() {
    // This agent never makes the file that the check looks for.
    let work = r#"commit greeting 'Add a greeting'
reply 'I made the file.'
while :; do
  next
  echo message >> "$here/more-messages"
  reply 'I can not repair it.'
done
"#;
    let repository = Repository::checked("check-fails-always", NEEDS_FIXED, work);
    let output = repository.run_in_order();
    let error = failed(&output);
    assert!(error.contains("failed 3 times"), "{error}");
    assert!(error.contains("and fixed.txt is not there"), "{error}");
    // Lira asked the agent to repair the change after the first and after
    // the second failure.
    assert_eq!(repository.order(), "check check check");
    assert_eq!(repository.kept("more-messages"), "message\nmessage");
    // The progress note says that Lira stopped, and why.
    let note = repository.gh("edited");
    let next = note.split("## Next").nth(1).unwrap();
    let next = next.split("## Decisions").next().unwrap();
    assert!(next.contains("Lira stopped"), "{note}");
    assert!(next.contains("the check failed 3 times"), "{note}");
    // No Reviewer read the change, and the pull request stays a draft.
    assert_eq!(repository.starts(), 1);
    assert!(error.contains(ADDRESS), "{error}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
    assert_eq!(repository.calls("pr ready"), Vec::<String>::new());
}

#[test]
fn the_agent_gets_the_end_of_a_long_output_of_the_check() {
    let check = r#"i=0
while [ "$i" -lt 3000 ]; do echo "line $i of the output of the check"; i=$((i + 1)); done
echo "the last line says what is wrong"
[ -e fixed.txt ]
"#;
    let repository = Repository::checked("check-long", check, WORK_THEN_REPAIR);
    passed(&repository.run_in_order());
    let second = kept_message(&repository, "second-message");
    assert!(
        second.contains("the last line says what is wrong"),
        "{second}"
    );
    assert!(second.contains("line 2999 of the output"), "{second}");
    assert!(second.contains("The start is left out."), "{second}");
    assert!(!second.contains("line 0 of the output"), "{second}");
    assert!(second.len() < 30_000, "{}", second.len());
}

#[test]
fn changes_that_are_in_no_commit_go_back_to_the_agent_before_the_check() {
    // The check would pass with the file that the agent did not commit.
    let work = r#"commit greeting 'Add a greeting'
echo fixed > fixed.txt
reply 'I made the files.'
next
printf '%s\n' "$line" > "$here/second-message"
git add fixed.txt
git commit --quiet --message 'Add the fixed file'
reply 'I committed it.'
"#;
    let repository = Repository::checked("not-in-a-commit", NEEDS_FIXED, work);
    passed(&repository.run_in_order());
    let second = kept_message(&repository, "second-message");
    assert!(second.contains("in no commit"), "{second}");
    assert!(second.contains("fixed.txt"), "{second}");
    // The check ran one time, on what is in the commits.
    assert_eq!(repository.order(), "check ready");
}

// spec(Steps): A Reviewer reads the change after the check passes.
#[test]
fn a_reviewer_reads_the_change_after_the_check_passes() {
    let repository = Repository::checked("review", NEEDS_FIXED, WORK_THEN_REPAIR);
    repository.reviewer("echo review >> \"$here/../order\"\nreply 'Good.\\n\\nReview: pass'\n");
    passed(&repository.run_in_order());
    // The check failed, then it passed, and only then a Reviewer started.
    assert_eq!(repository.order(), "check check review ready");
    assert_eq!(repository.starts(), 2);
    // The Reviewer got the request, the commits and the change.
    let message = repository.first_message(2);
    assert!(message.contains(REQUEST), "{message}");
    assert!(message.contains("Add a greeting"), "{message}");
    assert!(message.contains("Add the fixed file"), "{message}");
    assert!(message.contains("+++ b/greeting.txt"), "{message}");
    assert!(message.contains("+++ b/fixed.txt"), "{message}");
    // The folder of the Reviewer has the files of the change.
    assert!(message.contains("Review: pass"), "{message}");
}

#[test]
fn with_a_check_that_fails_no_reviewer_reads_the_change() {
    let work =
        "commit greeting 'Add a greeting'\nreply 'Done.'\nwhile :; do next; reply 'No.'; done\n";
    let repository = Repository::checked("no-review", NEEDS_FIXED, work);
    repository.reviewer("echo review >> \"$here/../order\"\nreply 'Review: pass'\n");
    failed(&repository.run_in_order());
    assert_eq!(repository.order(), "check check check");
}

// spec(Steps): A Reviewer can't change a file.
#[test]
fn a_reviewer_can_not_change_a_file() {
    let work = "git branch -m feat/reviewed\n";
    let repository = Repository::new("review-changes", CHECK, &format!("{work}{WORK}"));
    // This Reviewer asks if it can change a file. Then it changes a file of
    // the change, makes a new file and makes a commit, without the answer
    // that it needs.
    repository.reviewer(
        r#"say '{"jsonrpc":"2.0","id":9,"method":"session/request_permission","params":{"sessionId":"c-1","toolCall":{"toolCallId":"t-1","title":"Write greeting.txt","kind":"edit"},"options":[{"optionId":"yes","name":"Yes","kind":"allow_once"},{"optionId":"no","name":"No","kind":"reject_once"}]}}'
next
printf '%s\n' "$line" > "$here/answer"
ls > "$here/review-files"
echo changed > greeting.txt
echo new > new.txt
git add --all
git commit --quiet --message 'A commit of the Reviewer'
reply 'I changed it.\n\nReview: pass'
"#,
    );
    passed(&repository.run());
    // Lira said no.
    let answer = json::read(&repository.kept("answer")).unwrap();
    let expected =
        r#"{"jsonrpc":"2.0","id":9,"result":{"outcome":{"outcome":"selected","optionId":"no"}}}"#;
    assert_eq!(answer, json::read(expected).unwrap());
    // The Reviewer had the files of the change, in a folder of its own.
    assert!(repository.kept("review-files").contains("greeting.txt"));
    let folder = PathBuf::from(repository.kept("folder"));
    let review = PathBuf::from(repository.kept("review-folder"));
    assert_ne!(review, folder);
    // The change is as the agent left it: its files, its commits, and what
    // the remote has.
    assert_eq!(
        fs::read_to_string(folder.join("greeting.txt")).unwrap(),
        "greeting\n"
    );
    assert!(!folder.join("new.txt").exists());
    assert_eq!(git_ok(&folder, &["status", "--porcelain"]), "");
    assert_eq!(
        git_ok(&folder, &["log", "--format=%s", "-1"]),
        "Add a greeting"
    );
    assert_eq!(
        repository.on_remote("feat/reviewed"),
        Some(git_ok(&folder, &["rev-parse", "HEAD"]))
    );
    // The folder of the Reviewer is gone, and git has no record of it.
    assert!(!review.exists());
    let worktrees = git_ok(&repository.root, &["worktree", "list", "--porcelain"]);
    assert_eq!(worktrees.matches("worktree ").count(), 2, "{worktrees}");
    let branches = git_ok(&repository.root, &["branch", "--format=%(refname:short)"]);
    assert_eq!(branches, "feat/reviewed\nmain");
}

#[test]
fn lira_gives_the_findings_of_a_reviewer_to_the_agent() {
    let work = r#"commit greeting 'Add a greeting'
reply 'I made the file.'
next
printf '%s\n' "$line" > "$here/second-message"
commit repair 'Repair the greeting'
reply 'I repaired it.'
"#;
    let repository = Repository::checked("findings", "echo check >> \"$ORDER\"\n", work);
    // The first Reviewer has a finding, and the second one has none.
    repository.reviewer(
        r#"echo review >> "$here/../order"
if [ "$start" -eq 2 ]; then
  reply 'The greeting has no capital letter.\n\nReview: findings'
else
  reply 'Good now.\n\nReview: pass'
fi
"#,
    );
    let output = repository.run_in_order();
    passed(&output);
    let second = kept_message(&repository, "second-message");
    assert!(
        second.contains("The greeting has no capital letter."),
        "{second}"
    );
    // After the repair, the check runs again, and a new Reviewer reads the
    // change with the repair.
    assert_eq!(repository.order(), "check review check review ready");
    assert_eq!(repository.starts(), 3);
    let first = repository.first_message(2);
    let again = repository.first_message(3);
    assert!(!first.contains("+++ b/repair.txt"), "{first}");
    assert!(again.contains("+++ b/repair.txt"), "{again}");
    // The new Reviewer does not read what the one before it wrote.
    assert!(!again.contains("capital letter"), "{again}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("{ADDRESS}\n")
    );
}

#[test]
fn three_rounds_of_review_with_findings_end_the_run_with_a_failure() {
    let work = r#"commit greeting 'Add a greeting'
reply 'I made the file.'
while :; do
  next
  echo message >> "$here/more-messages"
  reply 'I do not agree.'
done
"#;
    let repository = Repository::new("findings-always", CHECK, work);
    repository.reviewer("reply 'It is still wrong.\\n\\nReview: findings'\n");
    let output = repository.run();
    let error = failed(&output);
    assert!(error.contains("3 rounds"), "{error}");
    assert!(error.contains("It is still wrong."), "{error}");
    // Three Reviewers read the change, and the agent got the findings of
    // the first two.
    assert_eq!(repository.starts(), 4);
    assert_eq!(repository.kept("more-messages"), "message\nmessage");
    let note = repository.gh("edited");
    let next = note.split("## Next").nth(1).unwrap();
    let next = next.split("## Decisions").next().unwrap();
    assert!(next.contains("Lira stopped"), "{note}");
    assert!(next.contains("3 rounds of review"), "{note}");
    assert!(error.contains(ADDRESS), "{error}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
}

#[test]
fn a_reviewer_that_does_not_say_pass_has_findings() {
    let work = r#"commit greeting 'Add a greeting'
reply 'I made the file.'
next
printf '%s\n' "$line" > "$here/second-message"
reply 'I looked at it.'
"#;
    let repository = Repository::new("no-verdict", CHECK, work);
    repository.reviewer(
        r#"if [ "$start" -eq 2 ]; then
  reply 'I am not sure about the greeting.'
else
  reply 'Review: pass'
fi
"#,
    );
    passed(&repository.run());
    let second = kept_message(&repository, "second-message");
    assert!(
        second.contains("I am not sure about the greeting."),
        "{second}"
    );
}

// spec(Format of the Lira file): | `models.reviewer` | Text | The AI model for the Reviewer. |
#[test]
fn the_reviewer_gets_the_ai_model_of_the_lira_file() {
    let lira_file = format!("{CHECK}[models]\nagent = \"model-a\"\nreviewer = \"model-r\"\n");
    let repository = Repository::new("review-model", &lira_file, WORK);
    passed(&repository.run());
    let model = |file: &str| {
        let text = fs::read_to_string(repository.harness.join(file)).unwrap();
        let new = json::read(text.lines().nth(1).unwrap()).unwrap();
        new.get("params")
            .and_then(|params| params.get("_meta"))
            .and_then(|meta| meta.get("claudeCode"))
            .and_then(|claude| claude.get("options"))
            .and_then(|options| options.get("model"))
            .and_then(Json::as_text)
            .map(str::to_string)
    };
    assert_eq!(model("received").as_deref(), Some("model-a"));
    assert_eq!(model("received-2").as_deref(), Some("model-r"));
}

#[test]
fn a_reviewer_whose_harness_ends_is_a_failure_and_leaves_no_folder() {
    let repository = Repository::new("review-ends", CHECK, WORK);
    repository.reviewer("echo 'the Reviewer broke' >&2\nexit 4\n");
    let output = repository.run();
    let error = failed(&output);
    assert!(error.contains("the Reviewer broke"), "{error}");
    assert!(error.contains(ADDRESS), "{error}");
    assert!(!Path::new(&repository.kept("review-folder")).exists());
    let worktrees = git_ok(&repository.root, &["worktree", "list", "--porcelain"]);
    assert_eq!(worktrees.matches("worktree ").count(), 2, "{worktrees}");
}

/// The description that an agent writes for a pull request.
const DESCRIPTION: &str = r"## Problem\n\nThe repository has no greeting.\n\n## Change\n\nIt adds greeting.txt.\n\n## Decisions\n\n- The greeting is one word.";

/// An agent that makes one commit, and writes the description of the pull
/// request when Lira asks for it. It keeps the message that asks.
fn work_and_describe(description: &str) -> String {
    format!(
        r#"commit greeting 'Add a greeting'
reply 'I made the file.'
next
printf '%s\n' "$line" > "$here/asks-for-the-description"
reply '{description}'
"#
    )
}

/// If a text has the three headings of a pull request that is ready, in
/// their order, and no other heading.
fn is_a_description(text: &str) -> bool {
    let headings: Vec<&str> = text.lines().filter(|line| line.starts_with('#')).collect();
    headings == ["## Problem", "## Change", "## Decisions"]
}

// spec(lira run): Lira runs the check and a Reviewer on the change, and marks the pull request ready for review.
#[test]
fn lira_marks_the_pull_request_ready_for_review() {
    let work = work_and_describe(DESCRIPTION);
    let repository = Repository::checked("ready", "echo check >> \"$ORDER\"\n", &work);
    repository.reviewer("echo review >> \"$here/../order\"\nreply 'Review: pass'\n");
    let output = repository.run_in_order();
    passed(&output);
    assert_eq!(repository.order(), "check review ready");
    assert_eq!(
        repository.calls("pr ready"),
        [format!("pr ready {ADDRESS}")]
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("{ADDRESS}\n")
    );
    assert!(
        errors(&output).contains("ready for review"),
        "{}",
        errors(&output)
    );
}

// spec(Pull requests): Lira marks a pull request ready for review only after the check and the Reviewer pass it.
#[test]
fn a_pull_request_is_ready_only_after_the_check_and_the_reviewer_pass_it() {
    let stubborn =
        "commit greeting 'Add a greeting'\nreply 'Done.'\nwhile :; do next; reply 'No.'; done\n";
    // The check never passes.
    let repository = Repository::checked("ready-no-check", NEEDS_FIXED, stubborn);
    failed(&repository.run_in_order());
    assert_eq!(repository.calls("pr ready"), Vec::<String>::new());
    assert_eq!(repository.calls("pr create").len(), 1);

    // The check passes, and the Reviewer never does.
    let repository = Repository::new("ready-no-review", CHECK, stubborn);
    repository.reviewer("reply 'Review: findings'\n");
    failed(&repository.run_in_order());
    assert_eq!(repository.calls("pr ready"), Vec::<String>::new());
    assert_eq!(repository.calls("pr create").len(), 1);

    // The check fails one time, and the Reviewer has findings one time.
    // The pull request is ready after the check and a Reviewer passed the
    // change as it is at the end.
    let work = r#"commit greeting 'Add a greeting'
reply 'I made the file.'
next
commit fixed 'Add the fixed file'
reply 'I repaired it.'
next
commit better 'Make it better'
reply 'I repaired it.'
"#;
    let repository = Repository::checked("ready-after", NEEDS_FIXED, work);
    repository.reviewer(
        r#"echo review >> "$here/../order"
if [ "$start" -eq 2 ]; then reply 'Review: findings'; else reply 'Review: pass'; fi
"#,
    );
    passed(&repository.run_in_order());
    assert_eq!(repository.order(), "check check review check review ready");
}

// spec(Pull requests): When a pull request is ready for review, its description has the headings `Problem`, `Change` and `Decisions`.
#[test]
fn the_description_of_a_ready_pull_request_has_its_three_headings() {
    let repository = Repository::new("described", CHECK, &work_and_describe(DESCRIPTION));
    passed(&repository.run());
    // The stand-in for `gh` kept the description that the pull request had
    // when Lira marked it ready.
    let description = repository.gh("description-when-ready");
    assert!(is_a_description(&description), "{description}");
    // It is what the agent wrote.
    assert!(
        description.contains("The repository has no greeting."),
        "{description}"
    );
    assert!(
        description.contains("- The greeting is one word."),
        "{description}"
    );
    // Lira asked the agent for these three headings.
    let asked = kept_message(&repository, "asks-for-the-description");
    for heading in ["`## Problem`", "`## Change`", "`## Decisions`"] {
        assert!(asked.contains(heading), "{asked}");
    }
}

#[test]
fn lira_writes_the_description_when_the_agent_does_not() {
    // One agent writes other headings, one writes no heading, and one
    // ends before it answers.
    let others = work_and_describe(r"## Summary\n\nIt adds a file.\n\n## Decisions\n\nNone.");
    let none = work_and_describe("It adds a file.");
    let gone = "commit greeting 'Add a greeting'\nreply 'I made the file.'\nnext\nexit 0\n";
    for (test, work) in [
        ("described-others", others.as_str()),
        ("described-none", none.as_str()),
        ("described-gone", gone),
    ] {
        let repository = Repository::new(test, CHECK, work);
        let output = repository.run();
        passed(&output);
        let description = repository.gh("description-when-ready");
        assert!(is_a_description(&description), "{test}: {description}");
        // The request, the commits, and what the agent said about its work.
        assert!(description.contains(REQUEST), "{test}: {description}");
        assert!(
            description.contains("- Add a greeting"),
            "{test}: {description}"
        );
        assert!(
            description.contains("I made the file."),
            "{test}: {description}"
        );
        assert_eq!(repository.calls("pr ready").len(), 1, "{test}");
    }
}

#[test]
fn text_around_the_description_of_the_agent_is_left_out() {
    let description = format!(r"Here is the description:\n\n{DESCRIPTION}");
    let repository = Repository::new("described-around", CHECK, &work_and_describe(&description));
    passed(&repository.run());
    let description = repository.gh("description-when-ready");
    assert!(description.starts_with("## Problem\n"), "{description}");
    assert!(is_a_description(&description), "{description}");
}

#[test]
fn a_commit_that_comes_with_the_description_is_checked_and_reviewed() {
    // The agent makes one more commit when Lira asks for the description.
    let work = r#"commit greeting 'Add a greeting'
reply 'I made the file.'
next
if [ ! -e late.txt ]; then
  commit late 'Add a late file'
fi
reply '## Problem\n\nNo greeting.\n\n## Change\n\nA greeting.\n\n## Decisions\n\nNone.'
next
reply '## Problem\n\nNo greeting.\n\n## Change\n\nA greeting, and a late file.\n\n## Decisions\n\nNone.'
"#;
    let repository = Repository::checked("described-late", "echo check >> \"$ORDER\"\n", work);
    repository.reviewer("echo review >> \"$here/../order\"\nreply 'Review: pass'\n");
    passed(&repository.run_in_order());
    // The check and a Reviewer passed the change with the late commit,
    // before the pull request was ready.
    assert_eq!(repository.order(), "check review check review ready");
    assert!(repository.first_message(3).contains("+++ b/late.txt"));
    let folder = PathBuf::from(repository.kept("folder"));
    let branch = repository.kept("branch");
    assert_eq!(
        repository.on_remote(&branch),
        Some(git_ok(&folder, &["rev-parse", "HEAD"]))
    );
    let description = repository.gh("description-when-ready");
    assert!(description.contains("and a late file"), "{description}");
}

// spec(lira run): A change from `lira run` always waits for you.
#[test]
fn a_change_from_lira_run_waits_for_you() {
    let repository = Repository::new("waits", CHECK, &work_and_describe(DESCRIPTION));
    passed(&repository.run());
    // The pull request is ready for review, and that is where Lira stops:
    // it does not merge the change, and it sets no label that lets an
    // agent merge it.
    let calls = repository.calls("");
    assert_eq!(calls.last().unwrap(), &format!("pr ready {ADDRESS}"));
    assert!(
        !calls.iter().any(|call| call.contains("merge")),
        "{calls:?}"
    );
    assert!(
        !calls.iter().any(|call| call.contains("label")),
        "{calls:?}"
    );
    // The main branch of the remote is as it was.
    assert_eq!(
        repository.on_remote("main"),
        Some(git_ok(&repository.seed, &["rev-parse", "HEAD"]))
    );
}

#[test]
fn a_pull_request_that_github_does_not_mark_ready_is_a_failure() {
    let repository = Repository::new("not-ready", CHECK, &work_and_describe(DESCRIPTION));
    // This `gh` does all but `pr ready`.
    let gh = GH.replace(
        "if [ \"$1 $2\" = \"pr ready\" ]; then",
        "if [ \"$1 $2\" = \"pr ready\" ]; then\n  echo 'GitHub says no' >&2\n  exit 1",
    );
    program(&repository.top.join("bin").join("gh"), &gh);
    let output = repository.run();
    let error = failed(&output);
    assert!(error.contains("GitHub says no"), "{error}");
    assert!(error.contains(ADDRESS), "{error}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
}
