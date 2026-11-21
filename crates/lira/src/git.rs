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
    let printed = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(output.status.success().then_some(printed))
}

/// The main branch as git names it here: the default branch of `origin`, or
/// the local `main` in a repository with no `origin`.
pub fn main_branch(folder: &Path) -> Result<String, String> {
    let head = [
        "symbolic-ref",
        "--quiet",
        "--short",
        "refs/remotes/origin/HEAD",
    ];
    if let Some(branch) = git(folder, &head)? {
        return Ok(branch);
    }
    for branch in ["origin/main", "main"] {
        if git(folder, &["rev-parse", "--verify", "--quiet", branch])?.is_some() {
            return Ok(branch.to_string());
        }
    }
    Err(
        "Lira can't find the main branch here. Run it in a git repository that has a branch `main`"
            .to_string(),
    )
}

/// The commit of the main branch that the change in `folder` is compared
/// with: the newest commit that both have.
pub fn base(folder: &Path, main: &str) -> Result<String, String> {
    git(folder, &["merge-base", main, "HEAD"])?.ok_or_else(|| {
        format!("Lira can't find the commit of `{main}` that this change starts from. Make a commit first")
    })
}
