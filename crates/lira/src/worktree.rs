//! A new worktree for a branch, in the data folder of Lira.

use crate::lira_file::LiraFile;
use crate::{git, process};
use std::fs;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

/// A worktree that Lira made.
pub struct Worktree {
    pub folder: PathBuf,
    /// The branch that the worktree started on.
    pub branch: String,
    /// The Lira file of the main branch that the worktree started from.
    pub file: LiraFile,
    /// The name of the remote that the main branch came from.
    pub remote: String,
    /// The name of the main branch on the remote, such as `main`.
    pub main: String,
    /// The commit of the main branch that the worktree started from.
    pub commit: String,
}

impl Worktree {
    /// Removes the worktree and its branch, with all that is in them. It is
    /// for a worktree that Lira made just now, and that no agent worked in.
    /// `repository` is the folder that the worktree was made from.
    pub fn remove(&self, repository: &Path) -> Result<(), String> {
        git::remove_worktree(repository, &self.folder, &self.branch)
    }
}

/// Makes a worktree of the repository in `repository`, on the new branch
/// `branch`. The branch starts from the newest main branch of the remote.
/// The `setup` command of the Lira file runs in the worktree before this
/// returns.
///
/// The worktree is `worktrees/<owner>/<repository>/<branch>` in `data`.
pub fn make(repository: &Path, data: &Path, branch: &str) -> Result<Worktree, String> {
    if !git::allows_branch(repository, branch)? {
        return Err(format!(
            "`{branch}` is not a name that git allows for a branch"
        ));
    }
    let newest = git::newest_main(repository)?;
    let (owner, name) = git::repository_of(repository, &newest.remote)?;
    let file = git::lira_file_at(repository, &newest.main, &newest.commit)?;
    if git::has_branch(repository, branch)? {
        return Err(format!(
            "the branch `{branch}` is already there. A new worktree needs a new branch"
        ));
    }
    // Two calls can ask for the same branch at the same time. Making the
    // folder and making the branch each work for one call only. So after
    // these two steps the folder and the branch are this call's own, and it
    // can remove them when a later step fails.
    let place = place(data, &[&owner, &name], branch)?;
    if let Err(error) = git::add_branch(repository, branch, &newest.commit) {
        let _ = fs::remove_dir(&place.folder);
        return Err(error);
    }
    let made = git::add_worktree(repository, &place.folder, branch)
        .and_then(|()| place.holds_the_worktree())
        .and_then(|()| match &file.setup {
            Some(command) => setup(command, &place.folder),
            None => Ok(()),
        });
    match made {
        Ok(()) => Ok(Worktree {
            folder: place.folder,
            branch: branch.to_string(),
            file,
            remote: newest.remote,
            main: newest.main.name,
            commit: newest.commit,
        }),
        Err(error) => Err(
            match git::remove_worktree(repository, &place.folder, branch) {
                Ok(()) => error,
                Err(wrote) => format!(
                    "{error}\nLira could not remove the worktree `{}` after that:\n{wrote}",
                    place.folder.display()
                ),
            },
        ),
    }
}

/// The worktree for the branch `branch` of a pull request whose work
/// continues. It is the old worktree of the branch when git still has it in
/// `data`, and a new one from the branch of the remote when it does not.
/// A commit that the remote has on the branch, and that the worktree does
/// not have, is taken first. Then the `setup` command runs in a new
/// worktree. It returns the worktree, and the commit that the remote has on
/// the branch.
pub fn continued(
    repository: &Path,
    data: &Path,
    branch: &str,
) -> Result<(Worktree, String), String> {
    if !git::allows_branch(repository, branch)? {
        return Err(format!(
            "`{branch}` is not a name that git allows for a branch"
        ));
    }
    let newest = git::newest_main(repository)?;
    let (owner, name) = git::repository_of(repository, &newest.remote)?;
    let file = git::lira_file_at(repository, &newest.main, &newest.commit)?;
    let on_remote = git::fetch_branch(repository, &newest.remote, branch)?;
    let remote = &newest.remote;
    let folder = match git::worktree_of(repository, branch)? {
        Some(folder) if folder.exists() => {
            let ours = data.join("worktrees").join(&owner).join(&name);
            let inside = match (folder.canonicalize(), ours.canonicalize()) {
                (Ok(folder), Ok(ours)) => folder.starts_with(ours),
                _ => false,
            };
            if !inside {
                return Err(format!(
                    "the branch `{branch}` is checked out in `{}`, which is no worktree of Lira. Check out another branch there, and run `lira continue` again",
                    folder.display()
                ));
            }
            take(&folder, branch, &on_remote, remote)?;
            folder
        }
        found => {
            // git keeps a record of a worktree whose folder is gone, and
            // makes no other worktree of its branch while it has it.
            if found.is_some() {
                git::prune_worktrees(repository)?;
            }
            let place = place(data, &[&owner, &name], branch)?;
            new_for(repository, place, branch, &on_remote, remote, &file)?
        }
    };
    let head = git::head(&folder)?;
    let commit = git::base_of(&folder, &newest.commit, &head)?.unwrap_or(newest.commit);
    let worktree = Worktree {
        folder,
        branch: branch.to_string(),
        file,
        remote: newest.remote,
        main: newest.main.name,
        commit,
    };
    Ok((worktree, on_remote))
}

/// Takes the commits that the remote has on the branch, up to the commit
/// `on_remote`, and that the worktree in `folder` does not have. The
/// worktree is on the branch.
fn take(folder: &Path, branch: &str, on_remote: &str, remote: &str) -> Result<(), String> {
    let head = git::head(folder)?;
    if git::is_in(folder, on_remote, &head)? {
        return Ok(());
    }
    if !git::is_in(folder, &head, on_remote)? {
        return Err(format!(
            "the worktree `{}` has commits on the branch `{branch}` that the remote `{remote}` does not have, and the remote has commits on it that the worktree does not have. Put them together on one branch, and run `lira continue` again",
            folder.display()
        ));
    }
    git::fast_forward(folder, on_remote)
}

/// Makes a new worktree of the branch `branch` in `place`. When the
/// repository does not have the branch, it starts at `on_remote`, the
/// commit of the remote. The worktree takes the commits of the remote, and
/// then the `setup` command runs in it. When a step fails, Lira removes the
/// worktree, and the branch when it made the branch.
fn new_for(
    repository: &Path,
    place: Place,
    branch: &str,
    on_remote: &str,
    remote: &str,
    file: &LiraFile,
) -> Result<PathBuf, String> {
    let new_branch = !git::has_branch(repository, branch)?;
    if new_branch && let Err(error) = git::add_branch(repository, branch, on_remote) {
        let _ = fs::remove_dir(&place.folder);
        return Err(error);
    }
    let made = git::add_worktree(repository, &place.folder, branch)
        .and_then(|()| place.holds_the_worktree())
        .and_then(|()| take(&place.folder, branch, on_remote, remote))
        .and_then(|()| match &file.setup {
            Some(command) => setup(command, &place.folder),
            None => Ok(()),
        });
    let Err(error) = made else {
        return Ok(place.folder);
    };
    let removed = match new_branch {
        true => git::remove_worktree(repository, &place.folder, branch),
        false => git::remove_only_worktree(repository, &place.folder, branch),
    };
    Err(match removed {
        Ok(()) => error,
        Err(wrote) => format!(
            "{error}\nLira could not remove the worktree `{}` after that:\n{wrote}",
            place.folder.display()
        ),
    })
}

/// The new, empty folder for a worktree.
struct Place {
    folder: PathBuf,
    /// The data folder, with each link in its path followed.
    data: PathBuf,
    /// The path from the data folder to the worktree.
    inside: PathBuf,
}

impl Place {
    /// Looks if the worktree is where it must be, after git made it. A
    /// folder above it can have become a link since Lira looked.
    fn holds_the_worktree(&self) -> Result<(), String> {
        match self.folder.canonicalize() {
            Ok(real) if real == self.data.join(&self.inside) => Ok(()),
            _ => Err(format!(
                "the worktree `{}` is not inside the data folder, so Lira does not use it",
                self.folder.display()
            )),
        }
    }
}

/// Makes the folder for the worktree of a branch, and the folders above it.
/// The folder of the worktree must be new.
///
/// Each folder between the data folder and the worktree must be a real
/// folder. A link there could put the worktree in another place.
fn place(data: &Path, repository: &[&str], branch: &str) -> Result<Place, String> {
    let failed = |folder: &Path, error: std::io::Error| {
        format!("Lira can't make the folder `{}`: {error}", folder.display())
    };
    let data = std::path::absolute(data).map_err(|e| failed(data, e))?;
    fs::create_dir_all(&data).map_err(|e| failed(&data, e))?;
    let real = data.canonicalize().map_err(|e| failed(&data, e))?;
    let parts = ["worktrees"]
        .into_iter()
        .chain(repository.iter().copied())
        .chain(branch.split('/'));
    let mut folder = data;
    let mut inside = PathBuf::new();
    let mut parts = parts.peekable();
    while let Some(part) = parts.next() {
        // A part is one name, and it does not go up. The code that calls
        // this checks the names, and this makes sure.
        let one_name = matches!(
            Path::new(part).components().collect::<Vec<_>>()[..],
            [Component::Normal(_)]
        );
        if !one_name {
            return Err(format!("`{part}` can't be the name of a folder"));
        }
        folder.push(part);
        inside.push(part);
        if parts.peek().is_none() {
            // Making the folder fails when something has the name already.
            return match fs::create_dir(&folder) {
                Ok(()) => Ok(Place {
                    folder,
                    data: real,
                    inside,
                }),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => Err(format!(
                    "the folder `{}` is already there. A new worktree needs a new folder",
                    folder.display()
                )),
                Err(error) => Err(failed(&folder, error)),
            };
        }
        match folder.symlink_metadata() {
            Ok(found) if found.is_dir() => {}
            Ok(_) => {
                return Err(format!(
                    "`{}` is not a folder of its own, so Lira makes no worktree in it",
                    folder.display()
                ));
            }
            Err(error) if error.kind() == ErrorKind::NotFound => match fs::create_dir(&folder) {
                Ok(()) => {}
                // Another call made it in between.
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
                Err(error) => return Err(failed(&folder, error)),
            },
            Err(error) => return Err(failed(&folder, error)),
        }
    }
    unreachable!("the path of a worktree has `worktrees` in it")
}

/// Runs the `setup` command in a worktree, with `sh -c`. The command has no
/// input. The error shows what the command wrote, to its output and its
/// error output, in the order that it wrote it.
fn setup(command: &str, folder: &Path) -> Result<(), String> {
    let failed = |problem: String| format!("the `setup` command, `{command}`, {problem}");
    let mut shell = Command::new("sh");
    shell.args(["-c", command]).current_dir(folder);
    let ended = process::run(shell, true).map_err(|e| failed(format!("did not run: {e}")))?;
    if ended.status.success() {
        return Ok(());
    }
    let status = ended.status;
    if ended.output.is_empty() {
        return Err(failed(format!("failed ({status}). It wrote nothing")));
    }
    let wrote = String::from_utf8_lossy(&ended.output);
    // One end of line less, and no other change to what the command wrote.
    let wrote = wrote.strip_suffix('\n').unwrap_or(&wrote);
    Err(failed(format!("failed ({status}). It wrote:\n{wrote}")))
}
