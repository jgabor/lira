//! The pull request of the work that an agent does in a worktree. Lira
//! pushes the commits of the agent, opens the pull request as a draft, and
//! keeps the progress note up to date.

use crate::worktree::Worktree;
use crate::{git, github};
use std::fs;

/// The file that holds a description on its way to GitHub. git keeps it for
/// the worktree, outside the files of the repository.
const DESCRIPTION: &str = "lira-description.md";

/// How many names Lira tries for the branch on the remote, when the remote
/// has a branch with the name of the agent already.
const NAMES: u32 = 9;

/// What the progress note says under `Next` while the agent works.
const WORKS: &str = "The agent works on the request.";

/// What the progress note says under `Decisions` while the agent works.
const LATER: &str = "The agent lists them when its work is complete.";

pub struct PullRequest<'a> {
    worktree: &'a Worktree,
    /// The branches that the repository had before the agent started. A
    /// branch of these is not the work of the agent.
    before: Vec<String>,
    /// The branch of the agent in the repository. It starts as the branch
    /// of the worktree, and the agent gives it another name.
    local: String,
    /// The name of the branch on the remote, after the first push.
    branch: Option<String>,
    /// The commit that the remote has on that branch.
    pushed: Option<String>,
    /// Each commit that Lira tried to push, and the name that it tried for
    /// the branch on the remote. The remote can have taken a push whose
    /// answer got lost.
    tried: Vec<String>,
    tried_as: Option<String>,
    /// The address of the pull request, after GitHub opened it.
    address: Option<String>,
    /// The description that GitHub has.
    described: String,
    /// What went wrong when Lira last looked, if something did. Lira tries
    /// again the next time.
    problem: Option<String>,
}

impl<'a> PullRequest<'a> {
    /// For the work in a new worktree, before the agent starts. There is no
    /// pull request yet.
    pub fn new(worktree: &'a Worktree) -> Result<PullRequest<'a>, String> {
        Ok(PullRequest {
            worktree,
            before: git::branches(&worktree.folder)?,
            local: worktree.branch.clone(),
            branch: None,
            pushed: None,
            tried: Vec::new(),
            tried_as: None,
            address: None,
            described: String::new(),
            problem: None,
        })
    }

    /// The address of the pull request, when GitHub opened one.
    pub fn address(&self) -> Option<&str> {
        self.address.as_deref()
    }

    /// The newest commit that the remote has of the work, when Lira pushed
    /// one.
    pub fn commit(&self) -> Option<&str> {
        self.pushed.as_deref()
    }

    /// The name that the branch of the agent has in the repository.
    pub fn local(&self) -> &str {
        &self.local
    }

    /// The name of the branch on the remote, when Lira pushed one.
    pub fn branch(&self) -> Option<&str> {
        self.branch.as_deref()
    }

    /// Looks for a new commit while the agent works. With one, it pushes
    /// the branch, and it opens the draft pull request or brings its
    /// progress note up to date. The agent does not wait for this, so a
    /// failure here does not stop the work: Lira tries again at the next
    /// look.
    pub fn look(&mut self) {
        let opened = self.address.is_some();
        self.problem = self.bring_up_to_date(WORKS, LATER).err();
        if let (false, Some(address)) = (opened, &self.address) {
            eprintln!("lira: the draft pull request is {address}");
        }
    }

    /// Brings the remote and the pull request up to date, with these texts
    /// under `Next` and `Decisions` in the progress note. With no commit of
    /// the agent, there is nothing to bring up to date, and no pull request.
    pub fn update(&mut self, next: &str, decisions: &str) -> Result<(), String> {
        let opened = self.address.is_some();
        self.bring_up_to_date(next, decisions)?;
        if let (false, Some(address)) = (opened, &self.address) {
            eprintln!("lira: the draft pull request is {address}");
        }
        Ok(())
    }

    /// Gives the pull request this description, and marks it ready for
    /// review. The remote must have the commit that Lira pushed last.
    pub fn ready(&mut self, description: &str) -> Result<(), String> {
        let (Some(address), Some(branch), Some(pushed)) =
            (self.address.clone(), &self.branch, &self.pushed)
        else {
            return Err("there is no pull request to mark ready for review".to_string());
        };
        // The check and a Reviewer passed the commit that Lira pushed. The
        // pull request is ready only with that commit.
        let folder = &self.worktree.folder;
        let remote = &self.worktree.remote;
        if git::remote_commit(folder, remote, branch)?.as_ref() != Some(pushed) {
            return Err(format!(
                "the branch `{branch}` has another commit on the remote `{remote}` than the one that the check and a Reviewer passed, so Lira does not mark the pull request ready for review"
            ));
        }
        self.describe(&address, description)?;
        github::mark_ready(&self.worktree.folder, &address)
    }

    /// Gives the pull request at `address` this description.
    fn describe(&mut self, address: &str, description: &str) -> Result<(), String> {
        let folder = &self.worktree.folder;
        let file = self.write(description)?;
        let sent = github::describe(folder, address, &file);
        let _ = fs::remove_file(&file);
        sent?;
        self.described = description.to_string();
        Ok(())
    }

    /// Writes a description to the file that `gh` reads it from.
    fn write(&self, description: &str) -> Result<std::path::PathBuf, String> {
        let file = git::own_file(&self.worktree.folder, DESCRIPTION)?;
        fs::write(&file, description).map_err(|error| {
            format!(
                "Lira can't write the description of the pull request to `{}`: {error}",
                file.display()
            )
        })?;
        Ok(file)
    }

    /// What went wrong at the last look, if something did.
    pub fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }

    /// Makes the remote and the pull request fit what the worktree has now.
    /// `next` and `decisions` are the text of these parts of the progress
    /// note.
    fn bring_up_to_date(&mut self, next: &str, decisions: &str) -> Result<(), String> {
        let folder = &self.worktree.folder;
        // The agent can make a commit while Lira looks. So Lira reads the
        // newest commit one time, and pushes and describes that commit.
        let commit = self.work()?;
        // The agent can put its work on a newer main branch. The commits
        // that it took from the main branch are not its work, so its work
        // starts at the newest commit that it has in common with the main
        // branch of the remote.
        let main = format!(
            "refs/remotes/{}/{}",
            self.worktree.remote, self.worktree.main
        );
        let start = git::base_of(folder, &main, &commit)?;
        let start = start.as_deref().unwrap_or(&self.worktree.commit);
        let subjects = git::subjects(folder, start, &commit)?;
        if subjects.is_empty() {
            return match &self.pushed {
                // The agent made no commit yet.
                None => Ok(()),
                // Lira removes no work from the remote.
                Some(_) => Err(
                    "the agent took its commits back, so its branch has no change now. The pull request still has the commits that Lira pushed before"
                        .to_string(),
                ),
            };
        }
        self.push(&commit)?;
        let description = note(&subjects, next, decisions);
        if self.address.is_some() && self.described == description {
            return Ok(());
        }
        if let Some(address) = self.address.clone() {
            return self.describe(&address, &description);
        }
        let file = self.write(&description)?;
        let branch = self.branch.as_deref().expect("the branch was pushed");
        let main = &self.worktree.main;
        let opened =
            github::open_draft(folder, main, branch, &subjects[0], &file).and_then(|draft| {
                // A pull request that is not new got its description in a call
                // before, and that can be an older one.
                let described = if draft.new {
                    Ok(())
                } else {
                    github::describe(folder, &draft.address, &file)
                };
                self.address = Some(draft.address);
                described
            });
        let _ = fs::remove_file(&file);
        opened?;
        self.described = description;
        Ok(())
    }

    /// The newest commit of the work of the agent: the commit of its
    /// branch. The worktree can be on another branch or on no branch, and
    /// that changes nothing here.
    fn work(&mut self) -> Result<String, String> {
        let folder = &self.worktree.folder;
        let commit = git::commit_of(folder, &self.local)?;
        let current = git::current_branch(folder)?;
        // A branch that the agent made, and that the worktree is on now.
        let new =
            current.filter(|current| *current != self.local && !self.before.contains(current));
        let moved = match (&commit, &new) {
            // The agent gave its branch a new name.
            (None, Some(_)) => true,
            // The agent made a new branch for its work, and left the first
            // one where it started. After the first push, the branch of
            // the pull request stays.
            (Some(commit), Some(_)) => *commit == self.worktree.commit && self.pushed.is_none(),
            _ => false,
        };
        if let (true, Some(new)) = (moved, new) {
            self.local = new;
            return git::commit_of(folder, &self.local)?.ok_or_else(|| self.lost());
        }
        commit.ok_or_else(|| self.lost())
    }

    /// The error for a branch of the agent that is gone.
    fn lost(&self) -> String {
        format!(
            "the branch `{}` of the agent is gone, and the worktree `{}` is on no new branch, so Lira can't find the work of the agent",
            self.local,
            self.worktree.folder.display()
        )
    }

    /// Makes the branch on the remote have this commit.
    fn push(&mut self, commit: &str) -> Result<(), String> {
        if self.pushed.as_deref() == Some(commit) {
            return Ok(());
        }
        let folder = &self.worktree.folder;
        let remote = &self.worktree.remote;
        let branch = match &self.branch {
            Some(branch) => branch.clone(),
            None => self.new_name()?,
        };
        // `new_name` can find a first push whose answer got lost.
        let before = self.pushed.clone();
        if before.as_deref() != Some(commit) {
            self.tried.push(commit.to_string());
            self.tried_as = Some(branch.clone());
            if let Err(error) = git::push(folder, remote, commit, &branch, before.as_deref()) {
                // The remote can have taken this push, or one before it,
                // and the answer got lost. Then the remote has a commit
                // that Lira tried to push, and Lira goes on from there.
                let there = git::remote_commit(folder, remote, &branch).ok().flatten();
                match there {
                    Some(there) if there == commit => {}
                    Some(there)
                        if self.tried.contains(&there) && Some(&there) != before.as_ref() =>
                    {
                        self.branch = Some(branch.clone());
                        self.pushed = Some(there.clone());
                        git::push(folder, remote, commit, &branch, Some(&there))?;
                    }
                    _ => return Err(error),
                }
            }
        }
        self.branch = Some(branch);
        self.pushed = Some(commit.to_string());
        Ok(())
    }

    /// The name for the branch on the remote: the name that the branch of
    /// the agent has now, or that name with a number when the remote has
    /// such a branch already.
    fn new_name(&mut self) -> Result<String, String> {
        let folder = &self.worktree.folder;
        let remote = &self.worktree.remote;
        // A first push whose answer got lost, under a name that the branch
        // of the agent had then.
        if let Some(tried_as) = self.tried_as.clone()
            && let Some(there) = git::remote_commit(folder, remote, &tried_as)?
            && self.tried.contains(&there)
        {
            self.pushed = Some(there);
            return Ok(tried_as);
        }
        let name = self.local.clone();
        let names = [name.clone()]
            .into_iter()
            .chain((2..=NAMES).map(|number| format!("{name}-{number}")));
        for other in names {
            if git::remote_commit(folder, remote, &other)?.is_none() {
                if other != name {
                    eprintln!(
                        "lira: the remote `{remote}` has a branch `{name}` already, so the branch is `{other}` there"
                    );
                }
                return Ok(other);
            }
        }
        Err(format!(
            "the remote `{remote}` has a branch `{name}` already, and `{name}-2` to `{name}-{NAMES}` too. Give the branch in `{}` another name",
            folder.display()
        ))
    }
}

/// The progress note: what is done, what is next, and what the agent
/// decided. What is done is the subject of each commit.
fn note(subjects: &[String], next: &str, decisions: &str) -> String {
    let done: String = subjects
        .iter()
        .map(|subject| format!("- {subject}\n"))
        .collect();
    format!("## Done\n\n{done}\n## Next\n\n{next}\n\n## Decisions\n\n{decisions}\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_progress_note_has_its_three_parts() {
        let subjects = ["Add a file".to_string(), "Test the file".to_string()];
        assert_eq!(
            note(&subjects, "The check.", "None."),
            "## Done\n\n- Add a file\n- Test the file\n\n## Next\n\nThe check.\n\n## Decisions\n\nNone.\n"
        );
    }
}
