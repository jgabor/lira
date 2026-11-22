//! What Lira asks git about the repository in a folder.

use crate::lira_file::{self, LiraFile};
use crate::process;
use std::ffi::OsStr;
use std::io::ErrorKind;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Runs git in `folder`. The inner result is what git printed, or what git
/// wrote to its error output when git failed. The outer error is for a git
/// that Lira could not run.
fn ask<S: AsRef<OsStr>>(folder: &Path, args: &[S]) -> Result<Result<String, String>, String> {
    let mut command = Command::new("git");
    command.args(args).current_dir(folder);
    // A hook of the repository can leave a process behind. Lira does not
    // wait for it.
    let ended = process::run(command, false)
        .map_err(|e| format!("couldn't run `git`: {e}. Install git, or add it to PATH"))?;
    Ok(if ended.status.success() {
        Ok(String::from_utf8_lossy(&ended.output).into_owned())
    } else {
        Err(String::from_utf8_lossy(&ended.errors).trim().to_string())
    })
}

/// Runs git in `folder`. When git fails, the error is what git wrote.
fn run<S: AsRef<OsStr>>(folder: &Path, args: &[S]) -> Result<String, String> {
    ask(folder, args)?
}

/// Runs git in `folder`, and returns what it prints when it succeeds.
fn git(folder: &Path, args: &[&str]) -> Result<Option<String>, String> {
    Ok(ask(folder, args)?.ok())
}

/// The main branch of the repository in a folder.
pub struct MainBranch {
    /// The full name that git has for it here, such as
    /// `refs/remotes/origin/main`. A full name can't mean a tag with the
    /// same short name.
    pub full: String,
    /// Its name on GitHub, such as `main`.
    pub name: String,
}

/// Finds the main branch: the default branch of a remote, `origin` first, or
/// the local `main` in a repository with no remote.
pub fn main_branch(folder: &Path) -> Result<MainBranch, String> {
    let remotes = git(folder, &["remote"])?.unwrap_or_default();
    let mut remotes: Vec<&str> = remotes.lines().collect();
    remotes.sort_by_key(|remote| *remote != "origin");
    // The name of a remote can have `/` in it, so the name of the branch is
    // what follows the whole name of the remote.
    for remote in &remotes {
        let start = format!("refs/remotes/{remote}/");
        let head = git(
            folder,
            &["symbolic-ref", "--quiet", &format!("{start}HEAD")],
        )?;
        if let Some(name) = head
            .as_deref()
            .and_then(|full| full.trim().strip_prefix(&start))
        {
            return Ok(MainBranch {
                full: format!("{start}{name}"),
                name: name.to_string(),
            });
        }
    }
    let with_main = remotes
        .iter()
        .map(|remote| format!("refs/remotes/{remote}/main"));
    for full in with_main.chain(["refs/heads/main".to_string()]) {
        if git(folder, &["rev-parse", "--verify", "--quiet", &full])?.is_some() {
            return Ok(MainBranch {
                full,
                name: "main".to_string(),
            });
        }
    }
    Err("Lira can't find the main branch here. Run it in a git repository that has a branch `main`, or a remote with a default branch".to_string())
}

/// The commit of the main branch that the change in `folder` is compared
/// with: the newest commit that both have.
pub fn base(folder: &Path, main: &str) -> Result<String, String> {
    let base = git(folder, &["merge-base", main, "HEAD"])?;
    base.map(|base| base.trim().to_string()).ok_or_else(|| {
        format!("Lira can't find the commit of `{main}` that this change starts from. Make a commit first")
    })
}

/// The Lira file, as the main branch has it.
pub fn lira_file(folder: &Path, main: &MainBranch) -> Result<LiraFile, String> {
    lira_file_at(folder, main, &main.full)
}

/// The Lira file, as one commit of the main branch has it.
pub fn lira_file_at(folder: &Path, main: &MainBranch, commit: &str) -> Result<LiraFile, String> {
    let text = file_on(folder, commit, lira_file::NAME)?.ok_or_else(|| {
        format!(
            "the main branch, `{}`, has no `{}`. Lira reads the Lira file from the main branch, so merge the pull request that adds it first",
            main.full,
            lira_file::NAME
        )
    })?;
    LiraFile::read(&text).map_err(|e| e.to_string())
}

/// The text of the file at `path`, from the root of the repository, as the
/// branch `main` has it. `None` when the branch has no such file.
pub fn file_on(folder: &Path, main: &str, path: &str) -> Result<Option<String>, String> {
    git(folder, &["show", &format!("{main}:{path}")])
}

/// The newest main branch of a remote.
pub struct Newest {
    pub main: MainBranch,
    /// The name of the remote: `origin`, or the first remote when there is
    /// no `origin`.
    pub remote: String,
    /// The newest commit of the main branch. Lira reads the Lira file from
    /// this commit and starts the worktree from it, so the two fit, also
    /// when the main branch gets a newer commit in between.
    pub commit: String,
}

/// Asks the remote which branch is its main branch now, and gets the newest
/// commits of that branch.
///
/// What the host knew about the remote before can be old: the main branch
/// can have new commits, and the remote can have another main branch.
pub fn newest_main(folder: &Path) -> Result<Newest, String> {
    let remotes = git(folder, &["remote"])?.unwrap_or_default();
    let mut remotes: Vec<&str> = remotes.lines().collect();
    remotes.sort_by_key(|remote| *remote != "origin");
    let Some(remote) = remotes.first() else {
        return Err(
            "this repository has no remote. Lira needs the remote, to start from the newest main branch"
                .to_string(),
        );
    };
    let failed = |wrote: String| {
        format!(
            "Lira can't get the newest main branch from the remote `{remote}`. git wrote:\n{wrote}"
        )
    };
    // After `--`, git takes a name that starts with `-` as a name.
    let head = run(folder, &["ls-remote", "--symref", "--", remote, "HEAD"]).map_err(failed)?;
    let name = head
        .lines()
        .find_map(|line| {
            line.strip_prefix("ref: refs/heads/")?
                .strip_suffix("\tHEAD")
        })
        .ok_or_else(|| {
            format!("the remote `{remote}` does not say which branch is its main branch")
        })?;
    let main = MainBranch {
        full: format!("refs/remotes/{remote}/{name}"),
        name: name.to_string(),
    };
    let branch = format!("+refs/heads/{name}:{}", main.full);
    run(
        folder,
        &["fetch", "--quiet", "--no-tags", "--", remote, &branch],
    )
    .map_err(failed)?;
    // The host keeps which branch is the main branch of the remote, and
    // `main_branch` reads it there.
    run(folder, &["remote", "set-head", "--", remote, name]).map_err(failed)?;
    let commit = run(
        folder,
        &[
            "rev-parse",
            "--verify",
            &format!("{}^{{commit}}", main.full),
        ],
    )
    .map_err(failed)?;
    Ok(Newest {
        main,
        remote: remote.to_string(),
        commit: commit.trim().to_string(),
    })
}

/// The repository of a remote, as `<owner>/<repository>`: the last two
/// parts of the address of the remote.
pub fn repository_of(folder: &Path, remote: &str) -> Result<(String, String), String> {
    let address = run(folder, &["remote", "get-url", "--", remote]).map_err(|wrote| {
        format!("Lira can't read the address of the remote `{remote}`: {wrote}")
    })?;
    let address = address.trim();
    owner_and_name(address).ok_or_else(|| {
        format!(
            "the address of the remote `{remote}`, `{address}`, does not end with `<owner>/<repository>`"
        )
    })
}

/// The last two parts of the address of a remote, such as
/// `https://github.com/owner/name.git` or `git@github.com:owner/name.git`.
fn owner_and_name(address: &str) -> Option<(String, String)> {
    let address = address.trim_end_matches('/');
    let address = address.strip_suffix(".git").unwrap_or(address);
    let mut parts = address.rsplit(['/', ':']);
    let name = parts.next()?;
    let owner = parts.next()?;
    // Each part becomes the name of a folder.
    let fits = |part: &str| {
        !part.is_empty()
            && part != "."
            && part != ".."
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    (fits(owner) && fits(name)).then(|| (owner.to_string(), name.to_string()))
}

/// If git allows this name for a branch.
pub fn allows_branch(folder: &Path, branch: &str) -> Result<bool, String> {
    // `check-ref-format --branch` also takes `@{-1}` and the like, which
    // name another branch. The full name takes only a real name. `HEAD`
    // is a name that git allows and that no person can then use.
    if branch.starts_with('-') || branch == "HEAD" {
        return Ok(false);
    }
    // git can make these two kinds of branch, and can't make a worktree of
    // them: it reads `@` as `HEAD`, and a name like that of a commit as a
    // commit.
    let like_a_commit =
        matches!(branch.len(), 40 | 64) && branch.chars().all(|c| c.is_ascii_hexdigit());
    if branch == "@" || like_a_commit {
        return Ok(false);
    }
    let full = format!("refs/heads/{branch}");
    Ok(git(folder, &["check-ref-format", &full])?.is_some())
}

/// The branch that the worktree in a folder is on. It is `None` when the
/// worktree is on no branch.
pub fn current_branch(folder: &Path) -> Result<Option<String>, String> {
    let branch = run(folder, &["branch", "--show-current"]).map_err(|wrote| {
        format!(
            "Lira can't read the branch of `{}`. git wrote:\n{wrote}",
            folder.display()
        )
    })?;
    let branch = branch.trim();
    Ok((!branch.is_empty()).then(|| branch.to_string()))
}

/// The commit that the worktree in a folder is at.
pub fn head(folder: &Path) -> Result<String, String> {
    run(folder, &["rev-parse", "--verify", "HEAD^{commit}"])
        .map(|commit| commit.trim().to_string())
        .map_err(|wrote| {
            format!(
                "Lira can't read the newest commit of `{}`. git wrote:\n{wrote}",
                folder.display()
            )
        })
}

/// The subject of each commit that `to` has and `from` does not have, the
/// oldest first.
pub fn subjects(folder: &Path, from: &str, to: &str) -> Result<Vec<String>, String> {
    let range = format!("{from}..{to}");
    let log = run(folder, &["log", "--reverse", "--format=%s", &range, "--"]).map_err(|wrote| {
        format!(
            "Lira can't read the commits of `{}`. git wrote:\n{wrote}",
            folder.display()
        )
    })?;
    Ok(log.lines().map(str::to_string).collect())
}

/// The changes of the worktree in a folder that are in no commit, one file
/// on each line, as `git status --short` prints them. It is empty when all
/// is in a commit.
pub fn changes(folder: &Path) -> Result<String, String> {
    // A setting of git can hide new files. Lira asks for them.
    run(folder, &["status", "--short", "--untracked-files=all"]).map_err(|wrote| {
        format!(
            "Lira can't read the changes of `{}`. git wrote:\n{wrote}",
            folder.display()
        )
    })
}

/// The change from the commit `from` to the commit `to`: all that `to` has
/// and that the newest commit of both does not have. With `names`, it is
/// the name of each file and how much of it changed, and not the lines.
pub fn diff(folder: &Path, from: &str, to: &str, names: bool) -> Result<String, String> {
    let range = format!("{from}...{to}");
    let form = if names { "--stat=200" } else { "--patch" };
    let args = ["diff", "--no-color", "--no-ext-diff", form, &range, "--"];
    run(folder, &args).map_err(|wrote| {
        format!(
            "Lira can't read the change in `{}`. git wrote:\n{wrote}",
            folder.display()
        )
    })
}

/// Makes a copy of the repository in the new folder `copy`, with the files
/// of a commit. The copy is a repository of its own: it shares no branch
/// with the repository in `folder`, and it knows no remote. So what git
/// does in the copy changes nothing outside it.
pub fn add_copy(folder: &Path, copy: &Path, commit: &str) -> Result<(), String> {
    let failed = |wrote: String| {
        format!(
            "Lira can't make a copy of the repository in `{}`. git wrote:\n{wrote}",
            copy.display()
        )
    };
    let top = run(folder, &["rev-parse", "--show-toplevel"]).map_err(failed)?;
    let clone = [
        OsStr::new("clone"),
        OsStr::new("--quiet"),
        OsStr::new("--no-checkout"),
        OsStr::new("--"),
        OsStr::new(top.trim_end_matches('\n')),
        copy.as_os_str(),
    ];
    run(folder, &clone).map_err(failed)?;
    run(copy, &["checkout", "--quiet", "--detach", commit]).map_err(failed)?;
    run(copy, &["remote", "remove", "origin"]).map_err(failed)?;
    Ok(())
}

/// The commit that a remote has on this branch now, or `None` when the
/// remote has no such branch.
pub fn remote_commit(folder: &Path, remote: &str, branch: &str) -> Result<Option<String>, String> {
    let full = format!("refs/heads/{branch}");
    let mut command = Command::new("git");
    command
        .args(["ls-remote", "--exit-code", "--heads", "--", remote, &full])
        .current_dir(folder);
    let ended = process::run(command, false)
        .map_err(|e| format!("couldn't run `git`: {e}. Install git, or add it to PATH"))?;
    let failed = || {
        format!(
            "Lira can't ask the remote `{remote}` for the branch `{branch}`. git wrote:\n{}",
            String::from_utf8_lossy(&ended.errors).trim()
        )
    };
    match ended.status.code() {
        // A line is the commit, a tab, and the full name of the branch.
        Some(0) => {
            let printed = String::from_utf8_lossy(&ended.output);
            let commit = printed.lines().find_map(|line| {
                let (commit, name) = line.split_once('\t')?;
                (name == full).then(|| commit.to_string())
            });
            commit.map(Some).ok_or_else(failed)
        }
        // With `--exit-code`, this status says that the remote has no such
        // branch.
        Some(2) => Ok(None),
        _ => Err(failed()),
    }
}

/// The commit of a branch of the repository, or `None` when the repository
/// has no such branch.
pub fn commit_of(folder: &Path, branch: &str) -> Result<Option<String>, String> {
    let full = format!("refs/heads/{branch}^{{commit}}");
    let commit = git(folder, &["rev-parse", "--verify", "--quiet", &full])?;
    Ok(commit.map(|commit| commit.trim().to_string()))
}

/// The name of each branch of the repository.
pub fn branches(folder: &Path) -> Result<Vec<String>, String> {
    let names = run(
        folder,
        &["for-each-ref", "--format=%(refname:short)", "refs/heads"],
    )
    .map_err(|wrote| {
        format!(
            "Lira can't read the branches of `{}`. git wrote:\n{wrote}",
            folder.display()
        )
    })?;
    Ok(names.lines().map(str::to_string).collect())
}

/// Pushes a commit to a branch of a remote. `before` is the commit that
/// the remote must have on that branch now, or `None` for a branch that
/// the remote must not have. So Lira replaces only what it pushed itself.
pub fn push(
    folder: &Path,
    remote: &str,
    commit: &str,
    branch: &str,
    before: Option<&str>,
) -> Result<(), String> {
    let full = format!("refs/heads/{branch}");
    let lease = format!("--force-with-lease={full}:{}", before.unwrap_or(""));
    let what = format!("{commit}:{full}");
    run(folder, &["push", "--quiet", &lease, "--", remote, &what])
        .map(|_| ())
        .map_err(|wrote| {
            format!("Lira can't push the branch `{branch}` to the remote `{remote}`. git wrote:\n{wrote}")
        })
}

/// The path of a file that git keeps for the worktree in a folder, outside
/// the files of the repository. git removes it with the worktree.
pub fn own_file(folder: &Path, name: &str) -> Result<PathBuf, String> {
    let path = run(
        folder,
        &["rev-parse", "--path-format=absolute", "--git-path", name],
    )
    .map_err(|wrote| {
        format!(
            "Lira can't find the folder that git keeps for `{}`. git wrote:\n{wrote}",
            folder.display()
        )
    })?;
    Ok(PathBuf::from(path.trim_end_matches('\n')))
}

/// If the repository has this branch.
pub fn has_branch(folder: &Path, branch: &str) -> Result<bool, String> {
    let full = format!("refs/heads/{branch}");
    Ok(git(folder, &["rev-parse", "--verify", "--quiet", &full])?.is_some())
}

/// Makes the new branch `branch` at a commit. The branch follows no other
/// branch. It fails when the branch is there already, so after it the
/// branch is Lira's own.
pub fn add_branch(folder: &Path, branch: &str, commit: &str) -> Result<(), String> {
    run(folder, &["branch", "--quiet", "--no-track", branch, commit])
        .map(|_| ())
        .map_err(|wrote| format!("Lira can't make the branch `{branch}`. git wrote:\n{wrote}"))
}

/// Makes a worktree of the branch `branch` in the empty folder `worktree`.
pub fn add_worktree(folder: &Path, worktree: &Path, branch: &str) -> Result<(), String> {
    let args = [
        OsStr::new("worktree"),
        OsStr::new("add"),
        OsStr::new("--quiet"),
        worktree.as_os_str(),
        OsStr::new(branch),
    ];
    run(folder, &args).map(|_| ()).map_err(|wrote| {
        format!(
            "Lira can't make the worktree `{}`. git wrote:\n{wrote}",
            worktree.display()
        )
    })
}

/// Removes the worktree of a branch and the branch, with all that is in
/// them. It is for a branch and a folder that Lira made just now, and no
/// other. When a part can't be removed, the error says why.
///
/// The branch says which worktree it is, and the folder does not: a
/// command can move a worktree, and put another one in its place.
pub fn remove_worktree(folder: &Path, worktree: &Path, branch: &str) -> Result<(), String> {
    let mut wrote = Vec::new();
    if let Some(now) = worktree_of(folder, branch)? {
        // git removes its record of the worktree, also when the folder is
        // gone.
        let args = [
            OsStr::new("worktree"),
            OsStr::new("remove"),
            OsStr::new("--force"),
            now.as_os_str(),
        ];
        if let Err(error) = run(folder, &args) {
            wrote.push(error);
        }
    }
    // When git had no record of the worktree, the empty folder that Lira
    // made is still there. `remove_dir` removes an empty folder only.
    match std::fs::remove_dir(worktree) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => wrote.push(format!(
            "Lira left the folder `{}`: {error}",
            worktree.display()
        )),
    }
    if has_branch(folder, branch)?
        && let Err(error) = run(folder, &["branch", "--quiet", "-D", branch])
    {
        wrote.push(error);
    }
    if wrote.is_empty() {
        Ok(())
    } else {
        Err(wrote.join("\n"))
    }
}

/// The folder of the worktree that has this branch, as git has it now.
fn worktree_of(folder: &Path, branch: &str) -> Result<Option<PathBuf>, String> {
    let mut command = Command::new("git");
    command
        .args(["worktree", "list", "--porcelain", "-z"])
        .current_dir(folder);
    let ended = process::run(command, false)
        .map_err(|e| format!("couldn't run `git`: {e}. Install git, or add it to PATH"))?;
    if !ended.status.success() {
        return Err(String::from_utf8_lossy(&ended.errors).trim().to_string());
    }
    Ok(worktree_with(&ended.output, branch))
}

/// Finds the worktree of a branch in what `git worktree list --porcelain
/// -z` prints: a `worktree <folder>` and then what is true for it, each
/// with a zero byte after it. A folder can have a name that is not UTF-8.
fn worktree_with(listed: &[u8], branch: &str) -> Option<PathBuf> {
    let wanted = format!("branch refs/heads/{branch}");
    let mut now = None;
    for part in listed.split(|byte| *byte == 0) {
        if let Some(path) = part.strip_prefix(b"worktree ") {
            now = Some(PathBuf::from(OsStr::from_bytes(path)));
        } else if part == wanted.as_bytes() {
            return now;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_worktree_of_a_branch() {
        let listed = b"worktree /w/main\0HEAD 1111\0branch refs/heads/main\0\0\
            worktree /w/data-\xff/feat\0HEAD 2222\0branch refs/heads/feat/a\0\0\
            worktree /w/loose\0HEAD 3333\0detached\0\0";
        assert_eq!(
            worktree_with(listed, "main"),
            Some(PathBuf::from("/w/main"))
        );
        assert_eq!(
            worktree_with(listed, "feat/a"),
            Some(PathBuf::from(OsStr::from_bytes(b"/w/data-\xff/feat")))
        );
        assert_eq!(worktree_with(listed, "feat"), None);
        assert_eq!(worktree_with(listed, "a"), None);
        assert_eq!(worktree_with(b"", "main"), None);
    }

    #[test]
    fn the_repository_is_the_last_two_parts_of_the_address() {
        for address in [
            "https://github.com/jgabor/lira.git",
            "https://github.com/jgabor/lira",
            "https://github.com/jgabor/lira/",
            "git@github.com:jgabor/lira.git",
            "ssh://git@github.com/jgabor/lira.git",
            "ssh://git@github.com:22/jgabor/lira",
            "/srv/git/jgabor/lira.git",
        ] {
            assert_eq!(
                owner_and_name(address),
                Some(("jgabor".to_string(), "lira".to_string())),
                "{address}"
            );
        }
        assert_eq!(
            owner_and_name("https://github.com/a-b_c/d.e.git"),
            Some(("a-b_c".to_string(), "d.e".to_string()))
        );
        for address in [
            "",
            "lira.git",
            "/lira.git",
            "https://github.com/jgabor/..",
            "https://github.com/../lira",
            "https://github.com/jgabor/.git",
            "https://github.com/jg abor/lira",
            "https://github.com//lira",
        ] {
            assert_eq!(owner_and_name(address), None, "{address}");
        }
    }
}
