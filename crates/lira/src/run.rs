//! `lira run`: an agent works on one request, in a new worktree. Lira opens
//! the pull request for its work, and runs the check and a Reviewer on the
//! change.

use crate::check::{self, Output};
use crate::harness::Harness;
use crate::pull_request::PullRequest;
use crate::worktree::{self, Worktree};
use crate::{agent, data, git, github, review};
use std::fs::{self, File};
use std::io::Read;
use std::path::Path;
use std::process;
use std::sync::mpsc;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

/// The instructions for the agent. The request follows them.
const INSTRUCTIONS: &str = include_str!("instructions.md");

/// After this many failed tries at the check, one after the other, Lira
/// stops.
const TRIES: usize = 3;

/// After this many rounds of review that end with findings, Lira stops.
const ROUNDS: usize = 3;

/// How much of the output of a failed check the agent gets, in bytes. It
/// is the end of the output, where a check says what is wrong.
const KEPT: usize = 20_000;

/// `lira run "<request>"`: makes a new worktree of the repository in
/// `folder`, and runs an agent on the request in it. It opens a draft pull
/// request at the first commit of the agent. When the reply of the agent
/// ends, it runs the check and a Reviewer on the change, and prints the
/// address of the pull request when both pass.
pub fn command(folder: &Path, request: &str) -> Result<(), String> {
    // A missing login stops the run here, before an agent works.
    github::require_login(folder)?;
    let data = data::folder()?;
    let made = worktree::make(folder, &data, &new_branch())?;
    let place = format!("`{}`", made.folder.display());
    eprintln!("lira: the agent works in {place}");

    let model = made.file.models.agent.as_deref();
    let (harness, conversation) = match agent::start(folder, &made.folder, model) {
        Ok(started) => started,
        // No agent worked in the worktree, so nothing in it is worth keeping.
        Err(error) => {
            return Err(match made.remove(folder) {
                Ok(()) => error,
                Err(wrote) => {
                    format!(
                        "{error}\nLira could not remove the worktree {place} after that:\n{wrote}"
                    )
                }
            });
        }
    };
    let mut run = Run {
        started: folder,
        made: &made,
        request,
        harness,
        conversation,
        pull_request: PullRequest::new(&made)?,
        said: String::new(),
        outputs: 0,
    };
    let address = match run.steps() {
        Ok(address) => address,
        Err(error) => {
            run.stop(&error);
            return Err(format!("{error}\n{}", run.left()));
        }
    };
    let branch = run.pull_request.branch().unwrap_or(&made.branch);
    eprintln!(
        "lira: the check and a Reviewer passed the change. It is on the branch `{branch}`, and its worktree is {place}"
    );
    eprintln!("{}", wrote(&run.said));
    println!("{address}");
    Ok(())
}

/// One run of `lira run`, after the agent started.
struct Run<'a> {
    /// The folder that `lira run` started in.
    started: &'a Path,
    made: &'a Worktree,
    request: &'a str,
    /// The harness of the agent. It runs until the run ends, so that the
    /// agent knows what it did when Lira gives it the output of the check or
    /// the findings of a Reviewer.
    harness: Harness,
    conversation: String,
    pull_request: PullRequest<'a>,
    /// The newest reply of the agent.
    said: String,
    /// How many files Lira wrote with the whole output of a failed check.
    /// Each file has a name of its own, so a message of before still names
    /// the output that it is about.
    outputs: usize,
}

impl Run<'_> {
    /// The steps after the start: the work of the agent, the check and the
    /// review, until the check and a Reviewer pass the change. It returns
    /// the address of the pull request.
    fn steps(&mut self) -> Result<String, String> {
        self.work(&message(&self.made.branch, self.request))?;
        let mut tries = 0;
        let mut rounds = 0;
        loop {
            // Check.
            if let Some(failure) = self.check()? {
                tries += 1;
                if tries == TRIES {
                    return Err(format!(
                        "the change did not pass the check {TRIES} times, one after the other, so Lira stops here\nThe last time, {}",
                        failure.for_you
                    ));
                }
                self.note("The agent repairs what the check found.")?;
                self.work(&failure.for_the_agent)?;
                continue;
            }
            tries = 0;
            // Review. The check passed one commit. Each time that Lira
            // brings the pull request up to date, it can push a newer
            // commit of the agent. Then the check reads the change again.
            let checked = self.pull_request.commit().map(str::to_string);
            self.note("A Reviewer reads the change.")?;
            if self.pull_request.commit() != checked.as_deref() {
                continue;
            }
            let review = self.review()?;
            if review.passed {
                self.note("Nothing. The check and a Reviewer passed the change.")?;
                if self.pull_request.commit() != checked.as_deref() {
                    continue;
                }
                return self.address();
            }
            rounds += 1;
            if rounds == ROUNDS {
                return Err(format!(
                    "{ROUNDS} rounds of review ended with findings, so Lira stops here\nThe last Reviewer wrote:\n\n{}\n",
                    review.text.trim()
                ));
            }
            self.note("The agent repairs what a Reviewer found.")?;
            self.work(&format!(
                "A Reviewer read your change, and it has findings. Test each finding against the code. Repair the real ones with the smallest change, and commit the repair. For each finding that you do not repair, say why at the end of your reply.\n\nThis is what the Reviewer wrote:\n\n{}\n",
                review.text.trim()
            ))?;
        }
    }

    /// Gives a message to the agent, and waits for its reply. While the
    /// agent works, Lira looks for a new commit each time a tool call ends.
    /// After the reply, all commits of the agent are in the pull request.
    fn work(&mut self, message: &str) -> Result<(), String> {
        let Run {
            harness,
            pull_request,
            conversation,
            ..
        } = self;
        // Lira looks for a new commit on a thread of its own. So the agent
        // does not wait for a permission while Lira talks to the remote or
        // to GitHub.
        let reply = thread::scope(|scope| {
            let (look, looks) = mpsc::channel();
            scope.spawn(move || {
                while looks.recv().is_ok() {
                    // One look is enough for all tool calls that ended
                    // since the look before.
                    while looks.try_recv().is_ok() {}
                    pull_request.look();
                }
            });
            harness.send_and_watch(
                conversation,
                message,
                // Nobody is there to answer, and the agent works in a
                // worktree of its own. So the answer to each permission
                // request is yes.
                &mut |_| true,
                // A tool call can be the first commit, or a later one.
                &mut || {
                    let _ = look.send(());
                },
            )
            // The thread ends here: nothing can ask for a look any more.
        });
        let reply = reply.map_err(|error| match self.pull_request.problem() {
            Some(problem) => format!("{error}\n{problem}"),
            None => error.to_string(),
        })?;
        self.said = reply.text;
        if let Some(why) = agent::stopped(&reply.stop) {
            return Err(format!(
                "the agent stopped before the end of its work: {why}\n{}",
                wrote(&self.said)
            ));
        }
        self.note("Lira runs the check.")
            .and_then(|()| self.address())
            .map(|_| ())
            .map_err(|error| format!("{error}\n{}", wrote(&self.said)))
    }

    /// Brings the pull request up to date, with this text under `Next` in
    /// its progress note. Under `Decisions` is the newest reply of the
    /// agent.
    fn note(&mut self, next: &str) -> Result<(), String> {
        let decisions = match self.said.trim() {
            "" => "The agent wrote nothing at the end of its work.",
            said => said,
        };
        self.pull_request.update(next, decisions)
    }

    /// Says in the progress note that Lira stopped, and why. GitHub is
    /// where the person reads how far the work is. The run failed already,
    /// so a failure here changes nothing.
    fn stop(&mut self, error: &str) {
        if self.pull_request.address().is_some() {
            let why = error.lines().next().unwrap_or_default();
            let _ = self.note(&format!(
                "Nothing. Lira stopped, and the pull request waits for you: {why}."
            ));
        }
    }

    /// The address of the pull request. It is an error when the agent made
    /// no commit, so that there is none.
    fn address(&self) -> Result<String, String> {
        self.pull_request
            .address()
            .map(str::to_string)
            .ok_or_else(|| "the agent made no commit, so Lira opened no pull request".to_string())
    }

    /// Runs the check on the change. It returns `None` when the check
    /// passes, and what to say about the failure when it does not.
    fn check(&mut self) -> Result<Option<Failure>, String> {
        let folder = &self.made.folder;
        // The check reads the files of the worktree, and the pull request
        // has the commits of the branch of the agent. So the two must be
        // the same: the worktree is at the newest commit of the pull
        // request, and all in it is in a commit.
        if let Some(failure) = self.moved("so the check waits", "so the check did not run")? {
            return Ok(Some(failure));
        }
        let changes = git::changes(folder)?;
        if !changes.trim().is_empty() {
            let changes = changes.trim_end();
            return Ok(Some(Failure {
                for_the_agent: format!(
                    "Your worktree has changes that are in no commit. Lira takes only what is in a commit, so the check waits. Commit what belongs to the change, and remove what does not. These are the changes, as `git status --short` prints them:\n\n{changes}\n"
                ),
                for_you: format!(
                    "the worktree had changes that are in no commit, so the check did not run:\n\n{changes}\n"
                ),
            }));
        }
        let main = format!("refs/remotes/{}/{}", self.made.remote, self.made.main);
        let base = git::base(folder, &main)?;
        let outcome = check::run(&self.made.file, folder, &base, Output::Kept)?;
        let command = &self.made.file.check;
        if outcome.passed {
            // A check can also go to another commit. Then it passed that
            // commit.
            let moved = self.moved(
                "after the check ran, so the check did not pass your commits",
                "after the check, so the check did not pass the commits of the pull request",
            )?;
            if moved.is_some() {
                return Ok(moved);
            }
            // A check can change files, as a command that formats them
            // does. Then it passed its own files, and not the commits.
            let changes = git::changes(folder)?;
            if changes.trim().is_empty() {
                return Ok(None);
            }
            let changes = changes.trim_end();
            return Ok(Some(Failure {
                for_the_agent: format!(
                    "The check of the repository, `{command}`, passed, and it left changes in your worktree that are in no commit. So it did not pass what is in your commits. Commit what belongs to the change, and make git ignore what the check only makes for itself. These are the changes, as `git status --short` prints them:\n\n{changes}\n"
                ),
                for_you: format!(
                    "the check, `{command}`, passed, and it left changes that are in no commit:\n\n{changes}\n"
                ),
            }));
        }
        let status = &outcome.status;
        let wrote = match end_of(&outcome.wrote, KEPT) {
            "" => "The check wrote nothing.\n".to_string(),
            end if end.len() < outcome.wrote.trim_end().len() => {
                // The start of a long output can say what is wrong. The
                // agent reads all of it in a file that git keeps for the
                // worktree, outside the files of the repository.
                self.outputs += 1;
                let name = format!("lira-check-output-{}.txt", self.outputs);
                let whole = git::own_file(folder, &name).and_then(|file| {
                    fs::write(&file, &outcome.wrote)
                        .map(|()| file)
                        .map_err(|error| error.to_string())
                });
                let start = match whole {
                    Ok(file) => format!(
                        "The start is left out here, and the file `{}` has all of it",
                        file.display()
                    ),
                    Err(_) => "The start is left out".to_string(),
                };
                format!("This is the end of what the check wrote. {start}.\n\n{end}\n")
            }
            end => format!("This is what the check wrote:\n\n{end}\n"),
        };
        Ok(Some(Failure {
            for_the_agent: format!(
                "The check of the repository failed. Lira ran `{command}` in your worktree, and it ended with {status}. Repair the change, and commit the repair.\n\n{wrote}"
            ),
            for_you: format!("the check, `{command}`, ended with {status}. {wrote}"),
        }))
    }

    /// Looks if the worktree is at the commit that Lira pushed for the pull
    /// request. The check reads the files of the worktree, and the pull
    /// request has the commits of the branch of the agent, so the two must
    /// be the same. When they are not, it returns what to say: `agent` and
    /// `you` end the sentences for the agent and for the person.
    fn moved(&self, agent: &str, you: &str) -> Result<Option<Failure>, String> {
        if self.pull_request.commit() == Some(git::head(&self.made.folder)?.as_str()) {
            return Ok(None);
        }
        let branch = self.pull_request.local();
        Ok(Some(Failure {
            for_the_agent: format!(
                "Your worktree is not at the newest commit of your branch, `{branch}`, {agent}. Lira takes the commits of that branch for the pull request. Check out `{branch}`, and put on it what belongs to the change.\n"
            ),
            for_you: format!(
                "the worktree was not at the newest commit of the branch `{branch}` of the agent {you}\n"
            ),
        }))
    }

    /// A new Reviewer reads the change as the pull request has it now.
    fn review(&mut self) -> Result<review::Review, String> {
        let folder = &self.made.folder;
        let main = format!("refs/remotes/{}/{}", self.made.remote, self.made.main);
        let from = git::base(folder, &main)?;
        // The check passed this commit just now: the one that Lira pushed
        // for the pull request.
        let Some(to) = self.pull_request.commit() else {
            return Err("the agent made no commit, so there is no change to review".to_string());
        };
        review::run(self.started, self.made, self.request, &from, to)
    }

    /// Where the work is, for an error.
    fn left(&self) -> String {
        let mut left = format!("What the agent did is in `{}`", self.made.folder.display());
        if let Some(branch) = self.pull_request.branch() {
            left.push_str(&format!(
                ", and on the branch `{branch}` of the remote `{}`",
                self.made.remote
            ));
        }
        if let Some(address) = self.pull_request.address() {
            left.push_str(&format!("\nThe pull request is a draft: {address}"));
        }
        left
    }
}

/// A check that did not pass.
struct Failure {
    /// The message that tells the agent to repair the change.
    for_the_agent: String,
    /// The end of the error for the person, when Lira stops.
    for_you: String,
}

/// What the agent wrote in a reply, for the person to read.
fn wrote(said: &str) -> String {
    match said.trim() {
        "" => "The agent wrote nothing".to_string(),
        text => format!("The agent wrote:\n\n{text}\n"),
    }
}

/// The end of a text: at most `most` bytes, from the start of a line when
/// the text has one there, and with no white space at its end.
fn end_of(text: &str, most: usize) -> &str {
    let text = text.trim_end();
    if text.len() <= most {
        return text;
    }
    let mut start = text.len() - most;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    let end = &text[start..];
    match end.find('\n') {
        Some(line) if line + 1 < end.len() => &end[line + 1..],
        _ => end,
    }
}

/// The message for the agent: the instructions, then the request as it is.
fn message(branch: &str, request: &str) -> String {
    format!("{}{request}", INSTRUCTIONS.replace("{branch}", branch))
}

/// A new name for the branch that a worktree of a request starts on:
/// `lira/run-` and 8 letters or digits. The agent gives the branch its
/// name later, so this name only has to be new.
fn new_branch() -> String {
    let mut bytes = [0; 4];
    let read = File::open("/dev/urandom").and_then(|mut file| file.read_exact(&mut bytes));
    let number = match read {
        Ok(()) => u32::from_be_bytes(bytes),
        // A host with no such file. The time and the number of this process
        // are different for each run.
        Err(_) => {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |time| time.subsec_nanos());
            nanos ^ process::id().rotate_left(16)
        }
    };
    format!("lira/run-{number:08x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_request_follows_the_instructions_as_it_is() {
        let text = message("lira/run-0a1b2c3d", "Do {branch}\n");
        assert!(text.contains("The branch is `lira/run-0a1b2c3d` now."));
        assert!(text.ends_with("The request of the user:\n\nDo {branch}\n"));
    }

    #[test]
    fn the_end_of_a_text_starts_at_a_line() {
        assert_eq!(end_of("one\ntwo\n\n", 100), "one\ntwo");
        assert_eq!(end_of("one\ntwo\nthree\n", 9), "three");
        assert_eq!(end_of("one\ntwo\nthree\n", 10), "two\nthree");
        // One long line has no start of a line in its end.
        assert_eq!(end_of("abcdefghij", 4), "ghij");
        // A cut does not go through a character.
        assert_eq!(end_of("aééé", 3), "é");
        assert_eq!(end_of("", 3), "");
    }

    #[test]
    fn a_new_branch_has_a_new_name() {
        let (one, other) = (new_branch(), new_branch());
        assert_ne!(one, other);
        assert_eq!(one.len(), "lira/run-".len() + 8);
    }
}
