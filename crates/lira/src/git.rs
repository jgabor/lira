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

/// The main branch as git names it here: the default branch of a remote,
/// `origin` first, or the local `main` in a repository with no remote.
pub fn main_branch(folder: &Path) -> Result<String, String> {
    let remotes = git(folder, &["remote"])?.unwrap_or_default();
    let mut remotes: Vec<&str> = remotes.lines().collect();
    remotes.sort_by_key(|remote| *remote != "origin");
    for remote in &remotes {
        let head = format!("refs/remotes/{remote}/HEAD");
        if let Some(branch) = git(folder, &["symbolic-ref", "--quiet", "--short", &head])? {
            return Ok(branch.trim().to_string());
        }
    }
    let with_main = remotes.iter().map(|remote| format!("{remote}/main"));
    for branch in with_main.chain(["main".to_string()]) {
        if git(folder, &["rev-parse", "--verify", "--quiet", &branch])?.is_some() {
            return Ok(branch);
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
