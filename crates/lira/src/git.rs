//! What Lira asks git about the repository in a folder.

use crate::lira_file::{self, LiraFile};
use std::path::Path;
use std::process::Command;

/// Runs git in `folder`, and returns what it prints when it succeeds.
fn git(folder: &Path, args: &[&str]) -> Result<Option<String>, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(folder)
        .output()
        .map_err(|e| format!("couldn't run `git`: {e}. Install git, or add it to PATH"))?;
    let printed = String::from_utf8_lossy(&output.stdout).into_owned();
    Ok(output.status.success().then_some(printed))
}

/// Runs git in `folder`. When git fails, the error is what git wrote.
fn run(folder: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(folder)
        .output()
        .map_err(|e| format!("couldn't run `git`: {e}. Install git, or add it to PATH"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
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
    let text = file_on(folder, &main.full, lira_file::NAME)?.ok_or_else(|| {
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

/// Asks the remote which branch is its main branch now, and gets the newest
/// commits of that branch. It returns the main branch and the name of the
/// remote: `origin`, or the first remote when there is no `origin`.
///
/// What the host knew about the remote before can be old: the main branch
/// can have new commits, and the remote can have another main branch.
pub fn newest_main(folder: &Path) -> Result<(MainBranch, String), String> {
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
    let head = run(folder, &["ls-remote", "--symref", remote, "HEAD"]).map_err(failed)?;
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
    run(folder, &["fetch", "--quiet", "--no-tags", remote, &branch]).map_err(failed)?;
    // The host keeps which branch is the main branch of the remote, and
    // `main_branch` reads it there.
    run(folder, &["remote", "set-head", remote, name]).map_err(failed)?;
    Ok((main, remote.to_string()))
}

/// The repository of a remote, as `<owner>/<repository>`: the last two
/// parts of the address of the remote.
pub fn repository_of(folder: &Path, remote: &str) -> Result<(String, String), String> {
    let address = run(folder, &["remote", "get-url", remote]).map_err(|wrote| {
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
    let full = format!("refs/heads/{branch}");
    Ok(git(folder, &["check-ref-format", &full])?.is_some())
}

/// If the repository has this branch.
pub fn has_branch(folder: &Path, branch: &str) -> Result<bool, String> {
    let full = format!("refs/heads/{branch}");
    Ok(git(folder, &["rev-parse", "--verify", "--quiet", &full])?.is_some())
}

/// Makes a worktree in `worktree`, on the new branch `branch`, which starts
/// at `start`. The branch follows no other branch.
pub fn add_worktree(
    folder: &Path,
    worktree: &Path,
    branch: &str,
    start: &str,
) -> Result<(), String> {
    let worktree = worktree.to_string_lossy();
    run(
        folder,
        &[
            "worktree",
            "add",
            "--quiet",
            "--no-track",
            "-b",
            branch,
            &worktree,
            start,
        ],
    )
    .map(|_| ())
    .map_err(|wrote| format!("Lira can't make the worktree `{worktree}`. git wrote:\n{wrote}"))
}

/// Removes a worktree and its branch, with all that is in them. It is for
/// a worktree and a branch that Lira made just now. When a part can't be
/// removed, the error is what git wrote.
pub fn remove_worktree(folder: &Path, worktree: &Path, branch: &str) -> Result<(), String> {
    let mut wrote = Vec::new();
    if worktree.exists()
        && let Err(error) = run(
            folder,
            &["worktree", "remove", "--force", &worktree.to_string_lossy()],
        )
    {
        wrote.push(error);
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

#[cfg(test)]
mod tests {
    use super::*;

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
