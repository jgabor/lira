//! The pull request of the work that an agent does in a worktree. Lira
//! pushes the commits of the agent, opens the pull request as a draft, and
//! keeps the progress note up to date.

use crate::worktree::Worktree;
use crate::{comment, git, github};
use std::fs;

/// The file that holds a description on its way to GitHub. git keeps it for
/// the worktree, outside the files of the repository.
const DESCRIPTION: &str = "lira-description.md";

/// The file that holds a comment on its way to GitHub. git keeps it for the
/// worktree, outside the files of the repository.
const COMMENT: &str = "lira-comment.md";

/// The label of a pull request whose agent stopped and asks.
pub const NEEDS_ANSWER: &str = "needs:answer";

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
    /// The comment with the request, until Lira wrote it on the pull
    /// request.
    request: Option<String>,
}

impl<'a> PullRequest<'a> {
    /// For the work on `request` in a new worktree, before the agent
    /// starts. There is no pull request yet.
    pub fn new(worktree: &'a Worktree, request: &str) -> Result<PullRequest<'a>, String> {
        Ok(PullRequest {
            request: Some(comment::request(request)),
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

    /// For the pull request at `address`, whose work continues in the
    /// worktree. Its branch on the remote is `branch`, and the worktree is
    /// on a branch with the same name.
    pub fn continued(
        worktree: &'a Worktree,
        address: &str,
        branch: &str,
    ) -> Result<PullRequest<'a>, String> {
        let folder = &worktree.folder;
        let pushed = git::remote_commit(folder, &worktree.remote, branch)?;
        Ok(PullRequest {
            worktree,
            before: git::branches(folder)?,
            local: worktree.branch.clone(),
            branch: Some(branch.to_string()),
            pushed,
            tried: Vec::new(),
            tried_as: None,
            address: Some(address.to_string()),
            described: String::new(),
            problem: None,
            taken: None,
            request: None,
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

    /// The agent stops and asks: Lira writes the question as a comment on
    /// the pull request, and then adds `needs:answer`. It tries both, and
    /// returns what went wrong.
    pub fn ask(&self, question: &str) -> Result<(), String> {
        let Some(address) = &self.address else {
            return Err("there is no pull request to write the question on".to_string());
        };
        let folder = &self.worktree.folder;
        let commented = self.comment(address, &comment::question(question));
        let labeled = github::add_label(folder, address, NEEDS_ANSWER);
        let mut problems = Vec::new();
        if let Err(error) = commented {
            problems.push(format!(
                "Lira could not write the question as a comment on the pull request: {error}"
            ));
        }
        if let Err(error) = labeled {
            problems.push(format!(
                "Lira could not add `{NEEDS_ANSWER}` to the pull request: {error}"
            ));
        }
        match problems.is_empty() {
            true => Ok(()),
            false => Err(problems.join("\n")),
        }
    }

    /// Writes a comment on the pull request at `address`.
    fn comment(&self, address: &str, text: &str) -> Result<(), String> {
        let folder = &self.worktree.folder;
        let file = git::own_file(folder, COMMENT)?;
        git::write_new(&file, text).map_err(|error| {
            format!(
                "Lira can't write the comment to `{}`: {error}",
                file.display()
            )
        })?;
        let sent = github::comment(folder, address, &file);
        let _ = fs::remove_file(&file);
        sent
    }

    /// Writes the comment with the request on the pull request, when there
    /// is a pull request and Lira did not write the comment yet.
    fn keep_request(&mut self) -> Result<(), String> {
        let (Some(address), Some(request)) = (&self.address, &self.request) else {
            return Ok(());
        };
        self.comment(address, request).map_err(|error| {
            format!("Lira could not write the request as a comment on the pull request: {error}")
        })?;
        self.request = None;
        Ok(())
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
        git::write_new(&file, description).map_err(|error| {
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

    /// The name of the branch of the agent, when the remote has a branch
    /// with that name already, so that Lira pushed nothing the last time.
    /// The branch on the remote is not the work of the agent, and Lira
    /// does not change it.
    pub fn taken(&self) -> Option<&str> {
        self.taken.as_deref()
    }

    /// Makes the remote and the pull request fit what the worktree has now,
    /// and writes the request as the first comment of a new pull request.
    /// `next` and `decisions` are the text of these parts of the progress
    /// note.
    fn bring_up_to_date(&mut self, next: &str, decisions: &str) -> Result<(), String> {
        let brought = self.fit(next, decisions);
        // A pull request that GitHub opened gets the request, also when its
        // description failed.
        let kept = self.keep_request();
        brought.and(kept)
    }

    /// Makes the remote and the pull request fit what the worktree has now.
    fn fit(&mut self, next: &str, decisions: &str) -> Result<(), String> {
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
