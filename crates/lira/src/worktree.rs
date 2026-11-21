//! A new worktree for a branch, in the data folder of Lira.

use crate::git;
use std::fs;
use std::io::{ErrorKind, Read};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// How long Lira still reads what the `setup` command wrote, after the
/// command ended. A process that the command left behind can hold the
/// output open for as long as it runs.
const LAST: Duration = Duration::from_millis(500);

/// Makes a worktree of the repository in `repository`, on the new branch
/// `branch`, and returns its folder. The branch starts from the newest main
/// branch of the remote. The `setup` command of the Lira file runs in the
/// worktree before this returns.
///
/// The worktree is `worktrees/<owner>/<repository>/<branch>` in `data`.
pub fn make(repository: &Path, data: &Path, branch: &str) -> Result<PathBuf, String> {
    if !git::allows_branch(repository, branch)? {
        return Err(format!(
            "`{branch}` is not a name that git allows for a branch"
        ));
    }
    let (main, remote) = git::newest_main(repository)?;
    let (owner, name) = git::repository_of(repository, &remote)?;
    let file = git::lira_file(repository, &main)?;
    if git::has_branch(repository, branch)? {
        return Err(format!(
            "the branch `{branch}` is already there. A new worktree needs a new branch"
        ));
    }
    let folder = place(data, &[&owner, &name], branch)?;
    // From here, the branch and the folder are Lira's own: neither was
    // there before. So Lira can remove them when a step fails.
    let made =
        git::add_worktree(repository, &folder, branch, &main.full).and_then(|()| {
            match &file.setup {
                Some(command) => setup(command, &folder),
                None => Ok(()),
            }
        });
    match made {
        Ok(()) => Ok(folder),
        Err(error) => Err(match git::remove_worktree(repository, &folder, branch) {
            Ok(()) => error,
            Err(wrote) => format!(
                "{error}\nLira could not remove the worktree `{}` after that. git wrote:\n{wrote}",
                folder.display()
            ),
        }),
    }
}

/// The folder for the worktree of a branch, which is not there yet. It
/// makes the folders above it.
///
/// Each folder between the data folder and the worktree must be a real
/// folder. A link there could put the worktree in another place.
fn place(data: &Path, repository: &[&str], branch: &str) -> Result<PathBuf, String> {
    let failed = |folder: &Path, error: std::io::Error| {
        format!("Lira can't make the folder `{}`: {error}", folder.display())
    };
    let data = std::path::absolute(data).map_err(|e| failed(data, e))?;
    fs::create_dir_all(&data).map_err(|e| failed(&data, e))?;
    let parts = ["worktrees"]
        .into_iter()
        .chain(repository.iter().copied())
        .chain(branch.split('/'));
    let mut folder = data;
    let mut parts = parts.peekable();
    while let Some(part) = parts.next() {
        // The names are checked before, and this makes sure of it: a part
        // is one name, and it does not go up.
        if !matches!(
            Path::new(part).components().collect::<Vec<_>>()[..],
            [Component::Normal(_)]
        ) {
            return Err(format!("`{part}` can't be the name of a folder"));
        }
        folder.push(part);
        let last = parts.peek().is_none();
        match folder.symlink_metadata() {
            Ok(_) if last => {
                return Err(format!(
                    "the folder `{}` is already there. A new worktree needs a new folder",
                    folder.display()
                ));
            }
            Ok(found) if found.is_dir() => {}
            Ok(_) => {
                return Err(format!(
                    "`{}` is not a folder of its own, so Lira makes no worktree in it",
                    folder.display()
                ));
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {
                if !last {
                    fs::create_dir(&folder).map_err(|e| failed(&folder, e))?;
                }
            }
            Err(error) => return Err(failed(&folder, error)),
        }
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
        Ok::<_, std::io::Error>((reader, child))
    };
    let (mut reader, mut child) = start().map_err(|e| failed(format!("did not start: {e}")))?;
    // A thread reads, so that the command never waits for Lira, and Lira
    // never waits for a process that the command left behind.
    let wrote = Arc::new(Mutex::new(Vec::new()));
    let (ended, reading_ended) = mpsc::channel();
    let keeper = Arc::clone(&wrote);
    thread::spawn(move || {
        let mut part = [0; 4096];
        loop {
            match reader.read(&mut part) {
                Ok(0) | Err(_) => break,
                Ok(count) => {
                    let Ok(mut wrote) = keeper.lock() else { break };
                    wrote.extend_from_slice(&part[..count]);
                }
            }
        }
        let _ = ended.send(());
    });
    let status = child
        .wait()
        .map_err(|e| failed(format!("did not end as a command does: {e}")))?;
    let _ = reading_ended.recv_timeout(LAST);
    if status.success() {
        return Ok(());
    }
    let wrote = wrote.lock().map(|wrote| wrote.clone()).unwrap_or_default();
    let wrote = String::from_utf8_lossy(&wrote);
    // One end of line less, and no other change to what the command wrote.
    let wrote = wrote.strip_suffix('\n').unwrap_or(&wrote);
    Err(failed(if wrote.is_empty() {
        format!("failed ({status}). It wrote nothing")
    } else {
        format!("failed ({status}). It wrote:\n{wrote}")
    }))
}
