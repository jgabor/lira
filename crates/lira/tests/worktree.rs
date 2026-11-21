//! Lira makes a worktree for a branch. Each test has a small repository
//! with a remote in a folder beside it, so no test asks GitHub.

use lira::worktree;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

struct Repository {
    /// The folder where a test writes the files of the main branch, and
    /// pushes them from. It stands in for the work of other people.
    seed: PathBuf,
    /// The remote: the repository `<owner>/<name>`.
    remote: PathBuf,
    owner: String,
    name: String,
    /// The repository on the host. Lira makes worktrees of it.
    root: PathBuf,
    /// The data folder of Lira.
    data: PathBuf,
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

impl Repository {
    /// A repository whose main branch has one commit, with this Lira file.
    /// With an empty text, the main branch has no Lira file.
    fn new(test: &str, lira_file: &str) -> Repository {
        Repository::named(test, lira_file, "owner", "name")
    }

    /// The same, for the repository `<owner>/<name>`.
    fn named(test: &str, lira_file: &str, owner: &str, name: &str) -> Repository {
        let top = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("worktree")
            .join(test);
        if top.exists() {
            fs::remove_dir_all(&top).unwrap();
        }
        let repository = Repository {
            seed: top.join("seed"),
            remote: top.join("remote").join(owner).join(format!("{name}.git")),
            owner: owner.to_string(),
            name: name.to_string(),
            root: top.join("repository"),
            data: top.join("data"),
            top: top.clone(),
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
            .join(&self.owner)
            .join(&self.name)
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
    let repository = Repository::named("folder", CHECK, "acme", "tool.box");
    let folder = repository.make("fix/login-timeout").unwrap();
    assert_eq!(
        folder,
        repository
            .data
            .join("worktrees/acme/tool.box/fix/login-timeout")
    );
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
    // A shell says in `$0` how it was run. With `sh -c`, that is `sh`.
    let lira_file = format!(
        "{CHECK}setup = 'word=shell; for n in 1 2; do echo \"$word $n $0\"; done > made.txt'\n"
    );
    let repository = Repository::new("setup-sh", &lira_file);
    let folder = repository.make("feat/sh").unwrap();
    assert_eq!(
        fs::read_to_string(folder.join("made.txt")).unwrap(),
        "shell 1 sh\nshell 2 sh\n"
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
        "{CHECK}setup = 'echo made > made.txt; echo changed > README.md; echo first line; echo second line >&2; echo third line; exit 7'\n"
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

#[test]
fn a_new_worktree_starts_from_the_main_branch_that_the_remote_has_now() {
    // The remote got another main branch after the host got its copy. The
    // old one is still there, and it is older.
    let repository = Repository::new("main-changed", CHECK);
    git_ok(&repository.seed, &["checkout", "--quiet", "-b", "trunk"]);
    fs::write(repository.seed.join("TRUNK.md"), "trunk\n").unwrap();
    git_ok(&repository.seed, &["add", "--all"]);
    git_ok(
        &repository.seed,
        &["commit", "--quiet", "--message", "trunk"],
    );
    let remote = repository.remote.to_str().unwrap();
    git_ok(&repository.seed, &["push", "--quiet", remote, "trunk"]);
    git_ok(
        &repository.remote,
        &["symbolic-ref", "HEAD", "refs/heads/trunk"],
    );
    let newest = git_ok(&repository.seed, &["rev-parse", "HEAD"]);

    let folder = repository.make("feat/trunk").unwrap();
    assert_eq!(git_ok(&folder, &["rev-parse", "HEAD"]), newest);
    // The host now knows the main branch of the remote.
    assert_eq!(
        git_ok(
            &repository.root,
            &["symbolic-ref", "refs/remotes/origin/HEAD"]
        ),
        "refs/remotes/origin/trunk"
    );
}

#[test]
fn a_repository_that_has_nothing_from_its_remote_yet_gets_a_worktree() {
    let repository = Repository::new("nothing-yet", CHECK);
    fs::remove_dir_all(&repository.root).unwrap();
    fs::create_dir_all(&repository.root).unwrap();
    git_ok(
        &repository.root,
        &["init", "--quiet", "--initial-branch", "work"],
    );
    let remote = repository.remote.to_str().unwrap();
    git_ok(&repository.root, &["remote", "add", "origin", remote]);
    let folder = repository.make("feat/first").unwrap();
    assert_eq!(
        git_ok(&folder, &["rev-parse", "HEAD"]),
        git_ok(&repository.seed, &["rev-parse", "HEAD"])
    );
}

#[test]
fn a_link_in_the_data_folder_can_not_move_a_worktree_out_of_it() {
    let repository = Repository::new("link", CHECK);
    let outside = repository.top.join("outside");
    fs::create_dir_all(&outside).unwrap();
    let beside = repository.data.join("worktrees/owner/name");
    fs::create_dir_all(&beside).unwrap();
    std::os::unix::fs::symlink(&outside, beside.join("feat")).unwrap();

    let error = repository.make("feat/example/deep").unwrap_err();
    assert!(error.contains("is not a folder of its own"), "{error}");
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
    assert!(!repository.has_branch("feat/example/deep"));
}

#[test]
fn a_folder_that_is_there_already_is_an_error_and_stays() {
    let repository = Repository::new("folder-there", CHECK);
    let folder = repository.worktree("feat/there");
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join("mine.txt"), "mine\n").unwrap();
    let error = repository.make("feat/there").unwrap_err();
    assert!(error.contains("is already there"), "{error}");
    assert_eq!(
        fs::read_to_string(folder.join("mine.txt")).unwrap(),
        "mine\n"
    );
    assert!(!repository.has_branch("feat/there"));
}

/// The path from one absolute folder to another, with `..` where it goes up.
fn relative(from: &Path, to: &Path) -> PathBuf {
    let from: Vec<_> = from.components().collect();
    let to: Vec<_> = to.components().collect();
    let same = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut path = PathBuf::new();
    for _ in same..from.len() {
        path.push("..");
    }
    for part in &to[same..] {
        path.push(part);
    }
    path
}

#[test]
fn a_data_folder_that_is_not_absolute_is_made_absolute() {
    let repository = Repository::new("relative", CHECK);
    let here = std::env::current_dir().unwrap();
    let data = relative(&here, &repository.data);
    assert!(data.is_relative());
    let folder = worktree::make(&repository.root, &data, "feat/relative").unwrap();
    assert!(folder.is_absolute(), "{folder:?}");
    assert!(folder.join("README.md").is_file());
    assert_eq!(
        folder.canonicalize().unwrap(),
        repository.worktree("feat/relative").canonicalize().unwrap()
    );
}

#[test]
fn the_error_shows_spaces_that_the_setup_command_wrote() {
    let repository = Repository::new(
        "setup-spaces",
        &format!("{CHECK}setup = \"printf '  \\\\n'; exit 7\"\n"),
    );
    let error = repository.make("feat/spaces").unwrap_err();
    assert!(error.ends_with("It wrote:\n  "), "{error:?}");

    let repository = Repository::new(
        "setup-newline",
        &format!("{CHECK}setup = \"printf '\\\\n'; exit 7\"\n"),
    );
    let error = repository.make("feat/newline").unwrap_err();
    assert!(error.ends_with("It wrote:\n"), "{error:?}");

    let repository = Repository::new("setup-silent", &format!("{CHECK}setup = \"exit 7\"\n"));
    let error = repository.make("feat/silent").unwrap_err();
    assert!(error.ends_with("It wrote nothing"), "{error:?}");
}

#[test]
fn a_setup_command_that_leaves_a_process_behind_does_not_hold_lira() {
    // The process that stays has the output of the command.
    for (name, end, good) in [("fails", "exit 7", false), ("passes", "exit 0", true)] {
        let repository = Repository::new(
            &format!("setup-stays-{name}"),
            &format!("{CHECK}setup = 'sleep 20 & echo started; {end}'\n"),
        );
        let started = Instant::now();
        let made = repository.make("feat/stays");
        assert!(started.elapsed() < Duration::from_secs(10), "{name}");
        assert_eq!(made.is_ok(), good, "{made:?}");
        if let Err(error) = made {
            assert!(error.contains("exit status: 7"), "{error}");
            assert!(error.contains("started"), "{error}");
            assert!(!repository.worktree("feat/stays").exists());
            assert!(!repository.has_branch("feat/stays"));
        }
    }
}

/// Gives the repository a hook that git runs when it made a worktree.
fn hook(repository: &Repository, script: &str) {
    let hooks = repository.root.join(".git").join("hooks");
    fs::create_dir_all(&hooks).unwrap();
    // A child process writes the file. If this process had it open for
    // writing while another test started a process, running the file could
    // fail with `Text file busy`.
    let mut writer = Command::new("sh")
        .args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"])
        .arg(hooks.join("post-checkout"))
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = writer.stdin.take().unwrap();
    input.write_all(script.as_bytes()).unwrap();
    drop(input);
    assert!(writer.wait().unwrap().success());
    // A setting of the host must not turn the hook off.
    git_ok(
        &repository.root,
        &["config", "core.hooksPath", hooks.to_str().unwrap()],
    );
}

#[test]
fn a_worktree_that_git_makes_and_then_refuses_is_removed() {
    // When the hook fails, git says so, and the worktree and the branch
    // are there. The second hook also leaves a process behind, which has
    // the output of git.
    let hooks = [
        ("hook", "#!/bin/sh\necho the hook says no >&2\nexit 9\n"),
        (
            "hook-stays",
            "#!/bin/sh\nsleep 20 &\necho the hook says no >&2\nexit 9\n",
        ),
    ];
    for (name, script) in hooks {
        let repository = Repository::new(name, CHECK);
        hook(&repository, script);
        let started = Instant::now();
        let error = repository.make("feat/hook").unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(10), "{name}");
        assert!(error.contains("the hook says no"), "{error}");
        assert!(!repository.worktree("feat/hook").exists());
        assert!(!repository.has_branch("feat/hook"));
        let listed = git_ok(&repository.root, &["worktree", "list", "--porcelain"]);
        assert!(!listed.contains("feat/hook"), "{listed}");
    }
}

#[test]
fn two_calls_for_one_branch_make_one_worktree() {
    // The calls start at the same time. At most one makes the worktree, and
    // the others must not remove what that one made.
    let repository = std::sync::Arc::new(Repository::new("two-calls", CHECK));
    let together = std::sync::Arc::new(std::sync::Barrier::new(6));
    let calls: Vec<_> = (0..6)
        .map(|_| {
            let repository = std::sync::Arc::clone(&repository);
            let together = std::sync::Arc::clone(&together);
            std::thread::spawn(move || {
                together.wait();
                repository.make("feat/same")
            })
        })
        .collect();
    let made: Vec<_> = calls.into_iter().map(|call| call.join().unwrap()).collect();
    let good = made.iter().filter(|made| made.is_ok()).count();
    assert!(good <= 1, "{made:?}");
    let folder = repository.worktree("feat/same");
    if good == 1 {
        assert!(folder.join("README.md").is_file(), "{made:?}");
        assert!(repository.has_branch("feat/same"), "{made:?}");
        assert_eq!(
            git_ok(&folder, &["symbolic-ref", "HEAD"]),
            "refs/heads/feat/same"
        );
    } else {
        assert!(!folder.exists(), "{made:?}");
        assert!(!repository.has_branch("feat/same"), "{made:?}");
    }
}

#[test]
fn a_data_folder_with_a_name_that_is_not_utf_8_gets_the_worktree() {
    use std::os::unix::ffi::OsStrExt;
    let mut repository = Repository::new(
        "not-utf-8",
        &format!("{CHECK}setup = \"echo made > made.txt\"\n"),
    );
    repository.data = repository
        .top
        .join(std::ffi::OsStr::from_bytes(b"data-\xff"));
    let folder = repository.make("feat/bytes").unwrap();
    assert_eq!(folder, repository.worktree("feat/bytes"));
    assert!(folder.join("README.md").is_file());
    assert_eq!(
        fs::read_to_string(folder.join("made.txt")).unwrap(),
        "made\n"
    );

    // A failure removes it again.
    repository.push("lira.toml", &format!("{CHECK}setup = \"exit 7\"\n"));
    repository.make("feat/more-bytes").unwrap_err();
    assert!(!repository.worktree("feat/more-bytes").exists());
    assert!(!repository.has_branch("feat/more-bytes"));
}

#[test]
fn a_name_of_the_remote_that_starts_with_a_dash_is_not_an_option_of_git() {
    let repository = Repository::new("dashes", CHECK);
    // The remote is `--quiet`, and its main branch is `-trunk`.
    let remote = repository.remote.to_str().unwrap();
    git_ok(
        &repository.seed,
        &["push", "--quiet", remote, "main:refs/heads/-trunk"],
    );
    git_ok(
        &repository.remote,
        &["symbolic-ref", "HEAD", "refs/heads/-trunk"],
    );
    git_ok(&repository.root, &["remote", "remove", "origin"]);
    git_ok(
        &repository.root,
        &["remote", "add", "--", "--quiet", remote],
    );

    let folder = repository.make("feat/dashes").unwrap();
    assert_eq!(
        git_ok(&folder, &["rev-parse", "HEAD"]),
        git_ok(&repository.seed, &["rev-parse", "HEAD"])
    );
}

#[test]
fn a_setup_command_that_moves_the_worktree_and_fails_leaves_no_branch() {
    let repository = Repository::new(
        "setup-moves",
        &format!("{CHECK}setup = 'mv \"$PWD\" \"$PWD.moved\"; exit 7'\n"),
    );
    repository.make("feat/moves").unwrap_err();
    assert!(!repository.worktree("feat/moves").exists());
    assert!(!repository.has_branch("feat/moves"));
    let listed = git_ok(&repository.root, &["worktree", "list", "--porcelain"]);
    assert!(!listed.contains("feat/moves"), "{listed}");
}
