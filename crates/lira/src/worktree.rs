//! A new worktree for a branch, in the data folder of Lira.

use crate::git;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Makes a worktree of the repository in `repository`, on the new branch
/// `branch`, and returns its folder. The branch starts from the newest main
/// branch. The `setup` command of the Lira file runs in the worktree before
/// this returns.
///
/// The worktree is `worktrees/<owner>/<repository>/<branch>` in `data`.
pub fn make(repository: &Path, data: &Path, branch: &str) -> Result<PathBuf, String> {
    if !git::allows_branch(repository, branch)? {
        return Err(format!(
            "`{branch}` is not a name that git allows for a branch"
        ));
    }
    let main = git::main_branch(repository)?;
    let Some(remote) = main.remote.as_deref() else {
        return Err(
            "this repository has no remote. Lira needs the remote, to start from the newest main branch"
                .to_string(),
        );
    };
    let (owner, name) = git::repository_of(repository, remote)?;
    git::fetch_main(repository, &main, remote)?;
    let file = git::lira_file(repository, &main)?;
    if git::has_branch(repository, branch)? {
        return Err(format!(
            "the branch `{branch}` is already there. A new worktree needs a new branch"
        ));
    }
    let folder = data.join("worktrees").join(owner).join(name).join(branch);
    git::add_worktree(repository, &folder, branch, &main.full)?;
    if let Some(command) = &file.setup
        && let Err(error) = setup(command, &folder)
    {
        return Err(match git::remove_worktree(repository, &folder, branch) {
            Ok(()) => error,
            Err(wrote) => format!(
                "{error}\nLira could not remove the worktree `{}` after that. git wrote:\n{wrote}",
                folder.display()
            ),
        });
    }
    Ok(folder)
}

/// Runs the `setup` command in a worktree, with `sh -c`. The command has no
/// input. The error shows what the command wrote, to its output and its
/// error output, in the order that it wrote it.
fn setup(command: &str, folder: &Path) -> Result<(), String> {
    let failed = |problem: String| format!("the `setup` command, `{command}`, {problem}");
    let start = || {
        let (reader, writer) = std::io::pipe()?;
        let child = Command::new("sh")
            .args(["-c", command])
            .current_dir(folder)
            .stdin(Stdio::null())
            .stdout(writer.try_clone()?)
            .stderr(writer)
            .spawn()?;
        // Only the command has the end for writing now, so the reading
        // ends when the command does.
        Ok::<_, std::io::Error>((reader, child))
    };
    let (mut reader, mut child) = start().map_err(|e| failed(format!("did not start: {e}")))?;
    let mut wrote = Vec::new();
    let read = reader.read_to_end(&mut wrote);
    let status = child
        .wait()
        .map_err(|e| failed(format!("did not end as a command does: {e}")))?;
    read.map_err(|e| failed(format!("wrote what Lira can't read: {e}")))?;
    if status.success() {
        return Ok(());
    }
    let wrote = String::from_utf8_lossy(&wrote);
    let wrote = wrote.trim_end();
    Err(failed(if wrote.is_empty() {
        format!("failed ({status}). It wrote nothing")
    } else {
        format!("failed ({status}). It wrote:\n{wrote}")
    }))
}
