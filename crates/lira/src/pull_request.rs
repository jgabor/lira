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
    /// The name of the branch on the remote, after the first push.
    branch: Option<String>,
    /// The commit that the remote has on that branch.
    pushed: Option<String>,
    /// The address of the pull request, after GitHub opened it.
    address: Option<String>,
    /// The description that GitHub has.
    described: String,
    /// What went wrong when Lira last looked, if something did. Lira tries
    /// again the next time.
    problem: Option<String>,
}

impl<'a> PullRequest<'a> {
    /// For the work in a new worktree. There is no pull request yet.
    pub fn new(worktree: &'a Worktree) -> PullRequest<'a> {
        PullRequest {
            worktree,
            branch: None,
            pushed: None,
            address: None,
            described: String::new(),
            problem: None,
        }
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
    /// review. The pull request must be up to date.
    pub fn ready(&mut self, description: &str) -> Result<(), String> {
        let Some(address) = self.address.clone() else {
            return Err("there is no pull request to mark ready for review".to_string());
        };
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
        let head = git::head(folder)?;
        let subjects = git::subjects(folder, &self.worktree.commit, &head)?;
        if subjects.is_empty() {
            // The agent made no commit yet.
            return Ok(());
        }
        if self.pushed.as_deref() != Some(&head) {
            let branch = match &self.branch {
                Some(branch) => branch.clone(),
                None => self.new_name()?,
            };
            let before = self.pushed.as_deref();
            git::push(folder, &self.worktree.remote, &head, &branch, before)?;
            self.branch = Some(branch);
            self.pushed = Some(head);
        }
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
        let opened = github::open_draft(folder, main, branch, &subjects[0], &file);
        let _ = fs::remove_file(&file);
        self.address = Some(opened?);
        self.described = description;
        Ok(())
    }

    /// The name for the branch on the remote: the name that the branch has
    /// in the worktree now, or that name with a number when the remote has
    /// such a branch already.
    fn new_name(&self) -> Result<String, String> {
        let folder = &self.worktree.folder;
        let Some(name) = git::current_branch(folder)? else {
            return Err(format!(
                "the worktree `{}` is on no branch, so Lira can't push the work of the agent",
                folder.display()
            ));
        };
        let remote = &self.worktree.remote;
        if !git::remote_has_branch(folder, remote, &name)? {
            return Ok(name);
        }
        for number in 2..=NAMES {
            let other = format!("{name}-{number}");
            if !git::remote_has_branch(folder, remote, &other)? {
                eprintln!(
                    "lira: the remote `{remote}` has a branch `{name}` already, so the branch is `{other}` there"
                );
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
