//! A new worktree for a branch, in the data folder of Lira.

use crate::lira_file::LiraFile;
use crate::{data, git, process, review};
use std::fs::{self, File, TryLockError};
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
    /// The lock of the work in the worktree. Lira holds it while its agent
    /// works there, and the system takes it away when Lira ends.
    _lock: File,
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
    let Some(lock) = lock(data, &[&owner, &name], branch)? else {
        return Err(format!(
            "an agent of Lira works on the branch `{branch}` already. A new worktree needs a new branch"
        ));
    };
    // Two calls can ask for the same branch at the same time. Making the
    // folder and making the branch each work for one call only. So after
    // these two steps the folder and the branch are this call's own, and it
    // can remove them when a later step fails.
    let place = place(data, &[&owner, &name], branch, false)?;
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
            _lock: lock,
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
/// `started_on` is the branch that the old worktree started on, when Lira
/// knows it. The old worktree is in the folder of that branch, also when
/// the agent gave the branch another name. When something that git does
/// not list is in the folder of the branch, a new worktree is beside it.
/// First a copy that a killed Reviewer left beside an old folder of the
/// work goes, when no worktree that git lists is in that folder.
/// A commit that the remote has on the branch, and that the worktree does
/// not have, is taken first. Then the `setup` command runs in a new
/// worktree. It returns the worktree, and the commit that the remote has on
/// the branch.
///
/// It is an error, and it changes nothing, while an agent of Lira works on
/// the pull request. The lock of the work is the lock of the branch that
/// the work started on.
pub fn continued(
    repository: &Path,
    data: &Path,
    branch: &str,
    started_on: Option<&str>,
) -> Result<(Worktree, String), String> {
    if !git::allows_branch(repository, branch)? {
        return Err(format!(
            "`{branch}` is not a name that git allows for a branch"
        ));
    }
    let newest = git::newest_main(repository)?;
    let (owner, name) = git::repository_of(repository, &newest.remote)?;
    let file = git::lira_file_at(repository, &newest.main, &newest.commit)?;
    let started = match started_on {
        Some(name) if git::allows_branch(repository, name)? => name,
        _ => branch,
    };
    let Some(lock) = lock(data, &[&owner, &name], started)? else {
        return Err(format!(
            "an agent of Lira works on the pull request of the branch `{branch}`, so Lira starts no second agent on it. Run `lira continue` again when that agent ends"
        ));
    };
    let on_remote = git::fetch_branch(repository, &newest.remote, branch)?;
    let remote = &newest.remote;
    let ours = data.join("worktrees").join(&owner).join(&name);
    // A Lira that was killed during a review can have left a copy beside a
    // folder of an old worktree that is gone. The work can continue in
    // another folder, so the copy goes now.
    for name in [started_on, Some(branch)].into_iter().flatten() {
        remove_left_copies(repository, &ours, name)?;
    }
    let folder = match git::worktree_of(repository, branch)? {
        Some(folder) if folder.exists() => {
            if !is_inside(&folder, &ours) {
                return Err(format!(
                    "the branch `{branch}` is checked out in `{}`, which is no worktree of Lira. Check out another branch there, and run `lira continue` again",
                    folder.display()
                ));
            }
            take(
                &folder,
                &worktree_named(&folder),
                branch,
                &on_remote,
                remote,
            )?;
            folder
        }
        _ => {
            // git keeps a record of a worktree whose folder is gone, and
            // makes no other worktree in that folder, or of its branch,
            // while it has it.
            // Lira removes only the records of its own worktrees, and not
            // one of yours whose folder is on a drive that is not there.
            let real = real_path(&ours);
            let inside: Vec<&Path> = [Some(ours.as_path()), real.as_deref()]
                .into_iter()
                .flatten()
                .collect();
            git::prune_worktrees(repository, &inside)?;
            // The old worktree can be in the folder of the branch that it
            // started on, or in the folder of the branch, and on another
            // branch or on no branch. Then it goes back to the branch. The
            // repository can have no such branch any more, when the agent
            // gave it another name. The folder that it started on comes
            // first: the folder of the branch can hold the worktree of an
            // earlier pull request on a branch with the same name. After
            // each folder comes the worktree beside it, `<name>~2` and so
            // on, that Lira made when something else was in the folder.
            let mut old = None;
            for name in [started_on, Some(branch)].into_iter().flatten() {
                let folder = ours.join(name);
                if is_at(&folder, &ours, name) && is_a_worktree_of(&folder, repository)? {
                    old = Some(folder);
                    break;
                }
                if let Some(folder) = beside(repository, &ours, name)? {
                    old = Some(folder);
                    break;
                }
            }
            if let Some(folder) = old {
                if !git::has_branch(repository, branch)? {
                    git::add_branch(repository, branch, &on_remote)?;
                }
                git::check_out(&folder, branch)?;
                take(
                    &folder,
                    &worktree_named(&folder),
                    branch,
                    &on_remote,
                    remote,
                )?;
                folder
            } else {
                // Something that git does not list can be in the folder of
                // the branch. Lira leaves it as it is, and makes the new
                // worktree beside it.
                let place = place(data, &[&owner, &name], branch, true)?;
                new_for(repository, place, branch, &on_remote, remote, &file)?
            }
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
        _lock: lock,
    };
    Ok((worktree, on_remote))
}

/// Takes the lock of the work that started on the branch `started_on`, in
/// a repository. It is `None` when another Lira holds the lock. The lock
/// is the file `locks/<owner>/<repository>/<started_on>.lock` in `data`.
/// The system takes the lock away when the file closes: when Lira ends,
/// also when it is killed. A child of Lira does not get the file, and an
/// agent does not see the folder of the locks.
fn lock(data: &Path, repository: &[&str], started_on: &str) -> Result<Option<File>, String> {
    let failed = |path: &Path, error: std::io::Error| {
        format!("Lira can't lock the file `{}`: {error}", path.display())
    };
    let mut path = data::locks(data);
    let parts: Vec<&str> = repository
        .iter()
        .copied()
        .chain(started_on.split('/'))
        .collect();
    for part in &parts {
        // A part is one name, and it does not go up.
        let one_name = matches!(
            Path::new(part).components().collect::<Vec<_>>()[..],
            [Component::Normal(_)]
        );
        if !one_name {
            return Err(format!("`{part}` can't be the name of a folder"));
        }
    }
    let (last, folders) = parts.split_last().expect("a lock has a branch");
    for part in folders {
        path.push(part);
    }
    fs::create_dir_all(&path).map_err(|error| failed(&path, error))?;
    // git allows no part of a branch that ends with `.lock`. So the file of
    // one branch is never the folder of another.
    path.push(format!("{last}.lock"));
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|error| failed(&path, error))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(error)) => Err(failed(&path, error)),
    }
}

/// The path `path` with each link in it followed. The end of the path
/// can be gone: then the links of the folder above it that is there are
/// followed, and the rest of the path is put after it. It is `None` when no
/// folder of the path is there.
fn real_path(path: &Path) -> Option<PathBuf> {
    let mut rest = Vec::new();
    let mut there = path;
    loop {
        if let Ok(real) = there.canonicalize() {
            return Some(rest.iter().rev().fold(real, |real, name| real.join(name)));
        }
        rest.push(there.file_name()?);
        there = there.parent()?;
    }
}

/// If `folder` is in the folder `ours`, with each link in both paths
/// followed.
fn is_inside(folder: &Path, ours: &Path) -> bool {
    match (folder.canonicalize(), ours.canonicalize()) {
        (Ok(folder), Ok(ours)) => folder.starts_with(ours),
        _ => false,
    }
}

/// If `folder` is the folder `branch` in the folder `ours`, with each link
/// in both paths followed. A link there to another folder is not. Each
/// part of `branch` is one name, and it does not go up.
fn is_at(folder: &Path, ours: &Path, branch: &str) -> bool {
    let names = !branch.is_empty()
        && Path::new(branch)
            .components()
            .all(|part| matches!(part, Component::Normal(_)));
    if !names {
        return false;
    }
    match (folder.canonicalize(), ours.canonicalize()) {
        (Ok(folder), Ok(ours)) => folder == ours.join(branch),
        _ => false,
    }
}

/// Removes each copy that a Reviewer left beside a folder where an old
/// worktree of the work can have been: the folder `name` in the folder
/// `ours`, and the folders `<name>~2`, `<name>~3`, and so on. A copy beside
/// a worktree that git lists stays: the Reviewer of that worktree removes
/// it. A name that goes out of `ours`, or a link above the folder, is not
/// used.
fn remove_left_copies(repository: &Path, ours: &Path, name: &str) -> Result<(), String> {
    let names = !name.is_empty()
        && Path::new(name)
            .components()
            .all(|part| matches!(part, Component::Normal(_)));
    if !names {
        return Ok(());
    }
    let folder = ours.join(name);
    let (Some(above), Some(last)) = (folder.parent(), folder.file_name()) else {
        return Ok(());
    };
    let path_above = Path::new(name).parent().unwrap_or(Path::new(""));
    let inside = match (above.canonicalize(), ours.canonicalize()) {
        (Ok(real), Ok(ours)) => real == ours.join(path_above),
        _ => false,
    };
    if !inside {
        return Ok(());
    }
    let Some(last) = last.to_str() else {
        return Ok(());
    };
    let Ok(entries) = fs::read_dir(above) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let entry = entry.file_name();
        let Some(worktree) = entry.to_str().and_then(|copy| copy.strip_suffix("-review")) else {
            continue;
        };
        let numbered = worktree
            .strip_prefix(last)
            .and_then(|rest| rest.strip_prefix('~'))
            .and_then(|number| number.parse::<u64>().ok())
            .is_some_and(|number| number >= 2 && worktree == format!("{last}~{number}"));
        let worktree = above.join(worktree);
        if (worktree == folder || numbered) && !is_a_worktree_of(&worktree, repository)? {
            review::remove_left_copy(&worktree)?;
        }
    }
    Ok(())
}

/// The worktree in `folder`, as an error names it.
fn worktree_named(folder: &Path) -> String {
    format!("the worktree `{}`", folder.display())
}

/// If `folder` is the top folder of a worktree that git has in its list of
/// worktrees of the repository in `repository`. Lira runs no git in
/// `folder` to know it: an agent can make a folder with a `.git` file and a
/// git folder of its own there.
fn is_a_worktree_of(folder: &Path, repository: &Path) -> Result<bool, String> {
    let Ok(folder) = folder.canonicalize() else {
        return Ok(false);
    };
    let listed = git::worktrees(repository)?;
    Ok(listed
        .iter()
        .any(|worktree| worktree.canonicalize().is_ok_and(|real| real == folder)))
}

/// The worktree with the lowest number of those in the folders
/// `<name>~2`, `<name>~3`, and so on, in the folder `ours`, that git has in
/// its list of worktrees of the repository in `repository`. Lira runs no
/// git in these folders to know it.
fn beside(repository: &Path, ours: &Path, name: &str) -> Result<Option<PathBuf>, String> {
    let mut found = None;
    for listed in git::worktrees(repository)? {
        let Some(number) = listed
            .file_name()
            .and_then(|last| last.to_str())
            .and_then(|last| last.rsplit_once('~'))
            .and_then(|(_, number)| number.parse::<u64>().ok())
            .filter(|number| *number >= 2)
        else {
            continue;
        };
        let numbered = format!("{name}~{number}");
        let folder = ours.join(&numbered);
        let same = matches!(
            (listed.canonicalize(), folder.canonicalize()),
            (Ok(listed), Ok(real)) if listed == real
        );
        let lower = found.as_ref().is_none_or(|(lowest, _)| number < *lowest);
        if same && lower && is_at(&folder, ours, &numbered) {
            found = Some((number, folder));
        }
    }
    Ok(found.map(|(_, folder)| folder))
}

/// Takes the commits that the remote has on the branch, up to the commit
/// `on_remote`, and that the worktree in `folder` does not have. The
/// worktree is on the branch. `whose` names where the commits of the branch
/// are, for an error.
fn take(
    folder: &Path,
    whose: &str,
    branch: &str,
    on_remote: &str,
    remote: &str,
) -> Result<(), String> {
    let head = git::head(folder)?;
    if git::is_in(folder, on_remote, &head)? {
        return Ok(());
    }
    if !git::is_in(folder, &head, on_remote)? {
        return Err(format!(
            "{whose} has commits on the branch `{branch}` that the remote `{remote}` does not have, and the remote has commits on it that {whose} does not have. Put them together on one branch, and run `lira continue` again"
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
        // The worktree is new, so the commits are those of the branch in
        // the repository, and Lira removes the worktree after an error.
        .and_then(|()| take(&place.folder, "the repository", branch, on_remote, remote))
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
/// The folder of the worktree must be new. With `beside`, when something is
/// in the folder of the branch already, the folder is the first new one of
/// `<branch>~2`, `<branch>~3`, and so on.
///
/// Each folder between the data folder and the worktree must be a real
/// folder. A link there could put the worktree in another place.
fn place(data: &Path, repository: &[&str], branch: &str, beside: bool) -> Result<Place, String> {
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
            // git allows no `~` in a branch, so a folder with `~<number>`
            // is never the folder of another branch.
            let mut number: u64 = 1;
            loop {
                let (folder, inside) = match number {
                    1 => (folder.clone(), inside.clone()),
                    _ => (
                        folder.with_file_name(format!("{part}~{number}")),
                        inside.with_file_name(format!("{part}~{number}")),
                    ),
                };
                match fs::create_dir(&folder) {
                    Ok(()) => {
                        return Ok(Place {
                            folder,
                            data: real,
                            inside,
                        });
                    }
                    Err(error) if error.kind() == ErrorKind::AlreadyExists && beside => {
                        number += 1;
                    }
                    Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                        return Err(format!(
                            "the folder `{}` is already there. A new worktree needs a new folder",
                            folder.display()
                        ));
                    }
                    Err(error) => return Err(failed(&folder, error)),
                }
            }
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
