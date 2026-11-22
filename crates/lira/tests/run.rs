//! Runs `lira run` in a small repository with a remote in a folder beside
//! it. A shell script stands in for the harness, and another one for `gh`,
//! so no test asks an AI model or GitHub.

use lira::json::{self, Json};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// What each stand-in for the harness starts with. `next` reads one line
/// from Lira and keeps it. `say` writes one line to Lira. The stand-in keeps
/// the folder and the branch that it starts in. Then it answers
/// `initialize` and `session/new`, and reads the message for the agent.
const START: &str = r#"#!/bin/sh
here="$(dirname "$0")"
next() { IFS= read -r line || exit 0; printf '%s\n' "$line" >> "$here/received"; }
say() { printf '%s\n' "$1"; }
pwd > "$here/folder"
git branch --show-current > "$here/branch"
next; say '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1}}'
next; say '{"jsonrpc":"2.0","id":2,"result":{"sessionId":"c-1"}}'
next
"#;

/// The reply of the agent, and its end.
const REPLY: &str = r#"say '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"c-1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"I made the file."}}}}'
say '{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}'
"#;

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
        // No test needs GitHub. This `gh` keeps what it is asked, and fails.
        program(
            &top.join("bin").join("gh"),
            "#!/bin/sh\necho \"$*\" >> \"$(dirname \"$0\")/calls\"\nexit 1\n",
        );
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
    let repository = Repository::new("request", CHECK, REPLY);
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
    assert_eq!(git_ok(&folder, &["rev-parse", "HEAD"]), newest);

    // The agent got the request.
    assert!(repository.message().contains(REQUEST));

    // Lira asked GitHub for no issue, and made none.
    let calls = fs::read_to_string(repository.top.join("bin").join("calls")).unwrap_or_default();
    assert!(!calls.contains("issue"), "{calls}");
}

// spec(Server): The data folder is `$XDG_DATA_HOME/lira`.
#[test]
fn the_data_folder_is_in_xdg_data_home() {
    let repository = Repository::new("xdg", CHECK, REPLY);
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
    let repository = Repository::new("home", CHECK, REPLY);
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
        let repository = Repository::new(test, CHECK, REPLY);
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
    let repository = Repository::new("no-data", CHECK, REPLY);
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
    let repository = Repository::new("named", CHECK, REPLY);
    let other = repository.top.join("other").join("my-harness");
    program(&other, &format!("{START}touch \"$here/started\"\n{REPLY}"));
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
    let repository = Repository::new("default", CHECK, REPLY);
    // A program with that name, in a folder of `PATH`.
    let default = repository.top.join("bin").join("claude-agent-acp");
    program(
        &default,
        &format!("{START}touch \"$here/started\"\n{REPLY}"),
    );
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
    let repository = Repository::new("relative-harness", CHECK, REPLY);
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
    let repository = Repository::new("no-harness", CHECK, REPLY);
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
    let repository = Repository::new("ends-early", CHECK, REPLY);
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
    let repository = Repository::new("branch", CHECK, REPLY);
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
    let repository = Repository::new("instructions", CHECK, REPLY);
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
    let repository = Repository::new("as-it-is", CHECK, REPLY);
    let request = "Use {branch} and \"quotes\"\n\nand a second paragraph: é";
    let output = repository.lira().args(["run", request]).output().unwrap();
    passed(&output);
    assert!(repository.message().ends_with(request));
}

#[test]
fn lira_says_on_which_branch_the_work_is() {
    let work = r#"git branch -m feat/add-greeting
echo hello > greeting.txt
git add greeting.txt
git commit --quiet --message 'Add a greeting'
"#;
    let repository = Repository::new("says-branch", CHECK, &format!("{work}{REPLY}"));
    let output = repository.run();
    passed(&output);
    let wrote = errors(&output);
    let folder = repository.kept("folder");
    assert!(wrote.contains("`feat/add-greeting`"), "{wrote}");
    assert!(wrote.contains(&format!("`{folder}`")), "{wrote}");
    // What the agent said is there for the person to read.
    assert!(wrote.contains("I made the file."), "{wrote}");
    // The output is for the address of the pull request, and no other thing.
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
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
    let repository = Repository::new("model", &lira_file, REPLY);
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
    let repository = Repository::new("no-model", CHECK, REPLY);
    passed(&repository.run());
    let new = repository.received(2);
    assert_eq!(new.get("params").unwrap().get("_meta"), None, "{new}");
}

#[test]
fn lira_says_yes_when_the_agent_asks_for_permission() {
    let work = r#"say '{"jsonrpc":"2.0","id":7,"method":"session/request_permission","params":{"sessionId":"c-1","toolCall":{"toolCallId":"t-1","title":"Write a.txt","kind":"edit"},"options":[{"optionId":"no","name":"No","kind":"reject_once"},{"optionId":"yes","name":"Yes","kind":"allow_once"}]}}'
next
"#;
    let repository = Repository::new("permission", CHECK, &format!("{work}{REPLY}"));
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
    let repository = Repository::new("usage", CHECK, REPLY);
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
    let repository = Repository::new("no-repository", CHECK, REPLY);
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
    let repository = Repository::new("inside", CHECK, REPLY);
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
