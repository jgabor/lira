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

/// The main branch, by the full name that git has for it here, such as
/// `refs/remotes/origin/main`: the default branch of a remote, `origin`
/// first, or the local `main` in a repository with no remote. A full name
/// can't mean a tag with the same short name.
pub fn main_branch(folder: &Path) -> Result<String, String> {
    let remotes = git(folder, &["remote"])?.unwrap_or_default();
    let mut remotes: Vec<&str> = remotes.lines().collect();
    remotes.sort_by_key(|remote| *remote != "origin");
    for remote in &remotes {
        let head = format!("refs/remotes/{remote}/HEAD");
        if let Some(branch) = git(folder, &["symbolic-ref", "--quiet", &head])? {
            return Ok(branch.trim().to_string());
        }
    }
    let with_main = remotes
        .iter()
        .map(|remote| format!("refs/remotes/{remote}/main"));
    for branch in with_main.chain(["refs/heads/main".to_string()]) {
        if git(folder, &["rev-parse", "--verify", "--quiet", &branch])?.is_some() {
            return Ok(branch);
        }
    }
    Err("Lira can't find the main branch here. Run it in a git repository that has a branch `main`, or a remote with a default branch".to_string())
}

/// The name of a branch on GitHub, from its full name in git.
pub fn name_on_github(branch: &str) -> &str {
    if let Some(name) = branch.strip_prefix("refs/heads/") {
        return name;
    }
    let remote = branch
        .strip_prefix("refs/remotes/")
        .and_then(|rest| rest.split_once('/'));
    remote.map_or(branch, |(_, name)| name)
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

#[cfg(test)]
mod tests {
    use super::name_on_github;

    #[test]
    fn the_name_on_github_has_no_remote_in_it() {
        assert_eq!(name_on_github("refs/remotes/origin/main"), "main");
        assert_eq!(
            name_on_github("refs/remotes/upstream/release/1"),
            "release/1"
        );
        assert_eq!(name_on_github("refs/heads/main"), "main");
    }
}
