//! What Lira asks git about the repository in a folder.

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

/// The text of the file at `path`, from the root of the repository, as the
/// branch `main` has it. `None` when the branch has no such file.
pub fn file_on(folder: &Path, main: &str, path: &str) -> Result<Option<String>, String> {
    git(folder, &["show", &format!("{main}:{path}")])
}
