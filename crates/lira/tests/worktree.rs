//! Lira makes a worktree for a branch. Each test has a small repository
//! with a remote in a folder beside it, so no test asks GitHub.

use lira::worktree;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Repository {
    /// The folder where the files of the main branch are written, and
    /// pushed from. It stands in for the work of other people.
    seed: PathBuf,
    /// The remote: the repository `owner/name`.
    remote: PathBuf,
    /// The repository on the host. Lira makes worktrees of it.
    root: PathBuf,
    /// The data folder of Lira.
    data: PathBuf,
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

impl Repository {
    /// A repository whose main branch has one commit, with this Lira file.
    /// With an empty text, the main branch has no Lira file.
    fn new(name: &str, lira_file: &str) -> Repository {
        let top = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("worktree")
            .join(name);
        if top.exists() {
            fs::remove_dir_all(&top).unwrap();
        }
        let repository = Repository {
            seed: top.join("seed"),
            remote: top.join("remote").join("owner").join("name.git"),
            root: top.join("repository"),
            data: top.join("data"),
        };
        fs::create_dir_all(&repository.seed).unwrap();
        fs::create_dir_all(repository.remote.parent().unwrap()).unwrap();
        git_ok(
            &repository.seed,
            &["init", "--quiet", "--initial-branch", "main"],
        );
        fs::write(repository.seed.join("README.md"), "first\n").unwrap();
        if !lira_file.is_empty() {
            fs::write(repository.seed.join("lira.toml"), lira_file).unwrap();
        }
        git_ok(&repository.seed, &["add", "--all"]);
        git_ok(
            &repository.seed,
            &["commit", "--quiet", "--message", "first"],
        );
        let remote = repository.remote.to_str().unwrap();
        git_ok(&top, &["clone", "--quiet", "--bare", "seed", remote]);
        git_ok(&top, &["clone", "--quiet", remote, "repository"]);
        repository
    }

    /// Puts a new commit on the main branch of the remote, with this file.
    /// The repository on the host does not have the commit yet.
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

    fn make(&self, branch: &str) -> Result<PathBuf, String> {
        worktree::make(&self.root, &self.data, branch)
    }

    fn worktree(&self, branch: &str) -> PathBuf {
        self.data
            .join("worktrees")
            .join("owner")
            .join("name")
            .join(branch)
    }

    fn has_branch(&self, branch: &str) -> bool {
        let full = format!("refs/heads/{branch}");
        git(&self.root, &["rev-parse", "--verify", "--quiet", &full]).is_some()
    }
}

const CHECK: &str = "check = \"true\"\n";

// spec(Steps): A new worktree starts from the newest main branch.
#[test]
fn a_new_worktree_starts_from_the_newest_main_branch() {
    let repository = Repository::new("newest", CHECK);
    let old = git_ok(&repository.root, &["rev-parse", "origin/main"]);
    let newest = repository.push("NEW.md", "newer\n");
    assert_ne!(old, newest);
    // The host has not heard of the newest commit.
    assert_eq!(git_ok(&repository.root, &["rev-parse", "origin/main"]), old);

    let folder = repository.make("feat/add-search").unwrap();

    assert_eq!(git_ok(&folder, &["rev-parse", "HEAD"]), newest);
    assert_eq!(
        fs::read_to_string(folder.join("NEW.md")).unwrap(),
        "newer\n"
    );
    assert_eq!(
        git_ok(&folder, &["symbolic-ref", "HEAD"]),
        "refs/heads/feat/add-search"
    );
    // The branch does not follow the main branch, so a push of it can't go
    // to the main branch.
    assert_eq!(
        git(&folder, &["rev-parse", "--abbrev-ref", "@{upstream}"]),
        None
    );
    // The folder where Lira was run keeps its branch and its files.
    assert_eq!(
        git_ok(&repository.root, &["symbolic-ref", "HEAD"]),
        "refs/heads/main"
    );
    assert_eq!(git_ok(&repository.root, &["rev-parse", "HEAD"]), old);
    assert!(!repository.root.join("NEW.md").exists());
}

// spec(Steps): A new worktree is the folder `worktrees/<owner>/<repository>/<branch>` in the data folder.
#[test]
fn a_new_worktree_is_in_the_data_folder() {
    let repository = Repository::new("folder", CHECK);
    let folder = repository.make("fix/login-timeout").unwrap();
    assert_eq!(folder, repository.worktree("fix/login-timeout"));
    assert!(folder.join("README.md").is_file());

    // A second worktree of the same repository is beside the first.
    let second = repository.make("docs").unwrap();
    assert_eq!(second, repository.worktree("docs"));
    assert!(second.join("README.md").is_file());
    assert!(folder.join("README.md").is_file());
}

// spec(Format of the Lira file): | `setup` | Text | A command that Lira runs in each new worktree. |
#[test]
fn lira_runs_the_setup_command_in_each_new_worktree() {
    let repository = Repository::new("setup", &format!("{CHECK}setup = \"pwd > where.txt\"\n"));
    for branch in ["feat/one", "feat/two"] {
        let folder = repository.make(branch).unwrap();
        let folder = folder.canonicalize().unwrap();
        assert_eq!(
            fs::read_to_string(folder.join("where.txt")).unwrap().trim(),
            folder.to_str().unwrap()
        );
    }
    // The command did not run in the folder where Lira was run.
    assert!(!repository.root.join("where.txt").exists());
}

#[test]
fn the_setup_command_is_that_of_the_newest_main_branch() {
    let repository = Repository::new(
        "setup-newest",
        &format!("{CHECK}setup = \"echo old > made.txt\"\n"),
    );
    repository.push(
        "lira.toml",
        &format!("{CHECK}setup = \"echo newest > made.txt\"\n"),
    );
    // A Lira file that is not on the main branch has no effect.
    fs::write(
        repository.root.join("lira.toml"),
        format!("{CHECK}setup = \"echo here > made.txt\"\n"),
    )
    .unwrap();
    let folder = repository.make("chore/setup").unwrap();
    assert_eq!(
        fs::read_to_string(folder.join("made.txt")).unwrap(),
        "newest\n"
    );
}

// spec(Steps): Lira runs the `setup` command with `sh -c`.
#[test]
fn lira_runs_the_setup_command_with_sh() {
    let lira_file = format!(
        "{CHECK}setup = 'word=shell; for n in 1 2; do echo \"$word $n\"; done > made.txt'\n"
    );
    let repository = Repository::new("setup-sh", &lira_file);
    let folder = repository.make("feat/sh").unwrap();
    assert_eq!(
        fs::read_to_string(folder.join("made.txt")).unwrap(),
        "shell 1\nshell 2\n"
    );
}

#[test]
fn a_lira_file_with_no_setup_runs_no_command() {
    let repository = Repository::new("no-setup", CHECK);
    let folder = repository.make("feat/none").unwrap();
    let files: Vec<String> = fs::read_dir(&folder)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name != ".git")
        .collect();
    let mut files = files;
    files.sort();
    assert_eq!(files, ["README.md", "lira.toml"]);
}

// spec(Steps): When the `setup` command fails, Lira removes the new worktree and its branch, and the error shows what the command wrote.
#[test]
fn a_setup_command_that_fails_is_an_error_that_shows_its_output() {
    let lira_file = format!(
        "{CHECK}setup = 'echo first line; echo second line >&2; echo third line; exit 7'\n"
    );
    let repository = Repository::new("setup-fails", &lira_file);
    let error = repository.make("feat/fails").unwrap_err();
    assert!(error.contains("`setup`"), "{error}");
    assert!(error.contains("exit status: 7"), "{error}");
    assert!(
        error.contains("first line\nsecond line\nthird line"),
        "{error}"
    );
    assert!(!repository.worktree("feat/fails").exists());
    assert!(!repository.has_branch("feat/fails"));
    let listed = git_ok(&repository.root, &["worktree", "list", "--porcelain"]);
    assert!(!listed.contains("feat/fails"), "{listed}");

    // The same branch works when the main branch has a good command.
    repository.push("lira.toml", &format!("{CHECK}setup = \"true\"\n"));
    repository.make("feat/fails").unwrap();
    assert!(repository.has_branch("feat/fails"));
}

#[test]
fn a_branch_that_is_there_already_is_an_error() {
    let repository = Repository::new(
        "branch-there",
        &format!("{CHECK}setup = \"echo ran >> ran.txt\"\n"),
    );
    let folder = repository.make("feat/twice").unwrap();
    let error = repository.make("feat/twice").unwrap_err();
    assert!(error.contains("`feat/twice`"), "{error}");
    assert!(error.contains("already"), "{error}");
    // The first worktree is as it was.
    assert_eq!(fs::read_to_string(folder.join("ran.txt")).unwrap(), "ran\n");
    assert!(repository.has_branch("feat/twice"));

    // A branch with no worktree is an error too, and it stays.
    git_ok(&repository.root, &["branch", "fix/mine"]);
    let error = repository.make("fix/mine").unwrap_err();
    assert!(error.contains("`fix/mine`"), "{error}");
    assert!(repository.has_branch("fix/mine"));
    assert!(!repository.worktree("fix/mine").exists());
}

#[test]
fn a_name_that_git_does_not_allow_for_a_branch_is_an_error() {
    let repository = Repository::new("branch-name", CHECK);
    for branch in [
        "",
        "-x",
        "a..b",
        "a b",
        "/a",
        "a/",
        "a//b",
        "../../outside",
        "a.lock",
        "HEAD",
    ] {
        let error = repository.make(branch).unwrap_err();
        assert!(
            error.contains("is not a name that git allows for a branch"),
            "{branch}: {error}"
        );
    }
    assert!(!repository.data.exists());
}

#[test]
fn a_main_branch_with_no_lira_file_is_an_error() {
    let repository = Repository::new("no-lira-file", "");
    let error = repository.make("feat/none").unwrap_err();
    assert!(error.contains("has no `lira.toml`"), "{error}");
    assert!(!repository.has_branch("feat/none"));
    assert!(!repository.data.exists());
}

#[test]
fn a_lira_file_with_an_error_is_an_error() {
    let repository = Repository::new("bad-lira-file", "setup = \"true\"\n");
    let error = repository.make("feat/none").unwrap_err();
    assert!(error.starts_with("lira.toml:"), "{error}");
    assert!(!repository.has_branch("feat/none"));
}

#[test]
fn a_repository_with_no_remote_is_an_error() {
    let repository = Repository::new("no-remote", CHECK);
    git_ok(&repository.root, &["remote", "remove", "origin"]);
    let error = repository.make("feat/none").unwrap_err();
    assert!(error.contains("has no remote"), "{error}");
    assert!(!repository.has_branch("feat/none"));
}

#[test]
fn a_remote_that_does_not_answer_is_an_error() {
    let repository = Repository::new("remote-gone", CHECK);
    fs::remove_dir_all(&repository.remote).unwrap();
    let error = repository.make("feat/none").unwrap_err();
    assert!(error.contains("newest"), "{error}");
    assert!(error.contains("`origin`"), "{error}");
    assert!(!repository.has_branch("feat/none"));
}
