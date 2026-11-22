//! The pull request of the work that an agent does in a worktree. Lira
//! pushes the commits of the agent, opens the pull request as a draft, and
//! keeps the progress note up to date.

use crate::worktree::Worktree;
use crate::{git, github};
use std::fs;

/// The file that holds a description on its way to GitHub. git keeps it for
/// the worktree, outside the files of the repository.
const DESCRIPTION: &str = "lira-description.md";

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
    /// The name of the branch of the agent, when Lira could not push it
    /// the last time because the remote has a branch with that name
    /// already.
    taken: Option<String>,
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
            taken: None,
        })
    }

    /// The address of the pull request, when GitHub opened one.
    pub fn address(&self) -> Option<&str> {
        self.address.as_deref()
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

    /// Looks one more time when the work of the agent is complete, and
    /// writes what the agent said at the end in the progress note. It
    /// returns the address of the pull request. `said` is the end of the
    /// reply of the agent.
    pub fn complete(&mut self, said: &str) -> Result<String, String> {
        let said = match said.trim() {
            "" => "The agent wrote nothing at the end of its work.",
            said => said,
        };
        let done = "Nothing. The work of the agent is complete.";
        self.bring_up_to_date(done, said)?;
        self.address
            .clone()
            .ok_or_else(|| "the agent made no commit, so Lira opened no pull request".to_string())
    }

    /// What went wrong at the last look, if something did.
    pub fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }

    /// The name of the branch of the agent, when the remote has a branch
    /// with that name already, so that Lira pushed nothing the last time.
    /// The branch on the remote is not the work of the agent, and Lira
    /// does not change it.
    pub fn taken(&self) -> Option<&str> {
        self.taken.as_deref()
    }

    /// Makes the remote and the pull request fit what the worktree has now.
    /// `next` and `decisions` are the text of these parts of the progress
    /// note.
    fn bring_up_to_date(&mut self, next: &str, decisions: &str) -> Result<(), String> {
        let folder = &self.worktree.folder;
        let was_taken = self.taken.take().is_some();
        // The agent can make a commit while Lira looks. So Lira reads the
        // newest commit one time, and pushes and describes that commit.
        let commit = self.work(was_taken)?;
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
        let file = git::own_file(folder, DESCRIPTION)?;
        fs::write(&file, &description).map_err(|error| {
            format!(
                "Lira can't write the description of the pull request to `{}`: {error}",
                file.display()
            )
        })?;
        let branch = self.branch.as_deref().expect("the branch was pushed");
        let sent = match &self.address {
            Some(address) => github::describe(folder, address, &file),
            None => {
                let main = &self.worktree.main;
                github::open_draft(folder, main, branch, &subjects[0], &file).and_then(|draft| {
                    // A pull request that is not new got its description
                    // in a call before, and that can be an older one.
                    let described = if draft.new {
                        Ok(())
                    } else {
                        github::describe(folder, &draft.address, &file)
                    };
                    self.address = Some(draft.address);
                    described
                })
            }
        };
        let _ = fs::remove_file(&file);
        sent?;
        self.described = description;
        Ok(())
    }

    /// The newest commit of the work of the agent: the commit of its
    /// branch. The worktree can be on another branch or on no branch, and
    /// that changes nothing here. `was_taken` says that Lira could not push
    /// the branch the last time, because the remote has its name already.
    fn work(&mut self, was_taken: bool) -> Result<String, String> {
        let folder = &self.worktree.folder;
        let commit = git::commit_of(folder, &self.local)?;
        let current = git::current_branch(folder)?;
        // A branch that the agent made, and that the worktree is on now.
        let new =
            current.filter(|current| *current != self.local && !self.before.contains(current));
        let moved = match (&commit, &new) {
            // The agent gave its branch a new name.
            (None, Some(_)) => true,
            // The agent made a new branch for its work. It left the first
            // one where it started, or it left one with a name that the
            // remote has already. Such a new branch is the work only when
            // it has the commits of the first one. After the first push,
            // the branch of the pull request stays.
            (Some(commit), Some(new)) => {
                let has_the_work = || {
                    let new = format!("refs/heads/{new}");
                    git::base_of(folder, &new, commit).map(|base| base.as_ref() == Some(commit))
                };
                self.pushed.is_none()
                    && (*commit == self.worktree.commit || (was_taken && has_the_work()?))
            }
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
    /// the agent has now. It is an error when the remote has such a branch
    /// already, and `taken` then has the name.
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
        if git::remote_commit(folder, remote, &name)?.is_some() {
            let error = format!(
                "the remote `{remote}` has a branch `{name}` already, so Lira did not push the branch of the agent"
            );
            self.taken = Some(name);
            return Err(error);
        }
        Ok(name)
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
