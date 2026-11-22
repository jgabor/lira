//! `lira run`: an agent works on one request, in a new worktree. Lira opens
//! the pull request for its work, and runs the check and a Reviewer on the
//! change.

use crate::check::{self, Output};
use crate::harness::{Harness, Reply};
use crate::pull_request::PullRequest;
use crate::sandbox::{self, Sandbox};
use crate::worktree::{self, Worktree};
use crate::{agent, data, git, github, review};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

/// The instructions for the agent. The request follows them.
const INSTRUCTIONS: &str = include_str!("instructions.md");

/// The message that asks the agent for the description of the pull
/// request.
const DESCRIBE: &str = include_str!("description.md");

/// After this many failed tries at the check, one after the other, the
/// agent stops and asks.
const TRIES: usize = 3;

/// After this many rounds of review that end with findings, the agent stops
/// and asks.
const ROUNDS: usize = 3;

/// How much of the output of a failed check the agent gets, in bytes. It
/// is the end of the output, where a check says what is wrong.
const KEPT: usize = 20_000;

/// How many times Lira tells the agent to give its branch another name,
/// when the remote has a branch with the name of the agent already.
const NAMES: usize = 3;

/// `lira run "<request>"`: makes a new worktree of the repository in
/// `folder`, and runs an agent on the request in it. It opens a draft pull
/// request at the first commit of the agent. When the reply of the agent
/// ends, it runs the check and a Reviewer on the change, and marks the
/// pull request ready for review when both pass. Then it prints the address
/// of the pull request.
pub fn command(folder: &Path, request: &str) -> Result<(), String> {
    // A missing login stops the run here, before an agent works.
    github::require_login(folder)?;
    // A host with no sandbox stops the run here too.
    sandbox::require()?;
    let data = data::folder()?;
    let made = worktree::make(folder, &data, &new_branch())?;
    let place = format!("`{}`", made.folder.display());
    eprintln!("lira: the agent works in {place}");

    let model = made.file.models.agent.as_deref();
    let started = sandbox_of(&made.folder).and_then(|mut sandbox| {
        agent::start(folder, &made.folder, &mut sandbox, &made.file, model)
            .map(|(harness, conversation)| (harness, conversation, sandbox))
    });
    let (harness, conversation, sandbox) = match started {
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
        sandbox,
        pull_request: PullRequest::new(&made)?,
        said: String::new(),
        outputs: 0,
    };
    let address = match run.steps() {
        Ok(address) => address,
        Err(stopped) => {
            let error = run.stop(stopped);
            return Err(format!("{error}\n{}", run.left()));
        }
    };
    let branch = run.pull_request.branch().unwrap_or(&made.branch);
    eprintln!(
        "lira: the check and a Reviewer passed the change, and the pull request is ready for review. The change is on the branch `{branch}`, and its worktree is {place}"
    );
    eprintln!("{}", wrote(&run.said));
    println!("{address}");
    Ok(())
}

/// The sandbox of the agent that works in the worktree `folder`. The agent
/// can change the worktree, and the git folder of the repository, where a
/// commit of the worktree goes. Lira runs git outside the sandbox later, so
/// the agent can't change the hooks and the configuration of git there, or
/// the files that say where git finds them.
fn sandbox_of(folder: &Path) -> Result<Sandbox, String> {
    let common = git::common_folder(folder)?;
    let hooks = common.join("hooks");
    // The git folders of the submodules of the repository. Each has a
    // configuration, which git reads when it goes into the submodule.
    let modules = common.join("modules");
    // An agent could make these folders when they are not there.
    for made in [&hooks, &modules] {
        fs::create_dir_all(made)
            .map_err(|error| format!("Lira can't make the folder `{}`: {error}", made.display()))?;
    }
    // A worktree has its own configuration of git when the repository says
    // so. The agent could make it when it is not there.
    // The main worktree has one too, in the shared git folder.
    let own_config = git::own_file(folder, "config.worktree")?;
    let main_config = common.join("config.worktree");
    // git in the main worktree reads `commondir` in the shared git folder
    // when it is there, and it then finds the shared git folder where that
    // file says. An empty file is an error to git, so the file names the
    // folder that it is in.
    let main_common = common.join("commondir");
    for (file, content) in [(&own_config, ""), (&main_config, ""), (&main_common, ".\n")] {
        make_whole(file, content)
            .map_err(|error| format!("Lira can't make the file `{}`: {error}", file.display()))?;
    }
    let own_folder = git::own_folder(folder)?;
    let mut sandbox = Sandbox::new();
    sandbox
        .writable(folder)
        .writable(&common)
        .read_only(&hooks)
        .read_only(&common.join("config"))
        .read_only(&main_config)
        .read_only(&main_common)
        .read_only(&modules)
        // The git folders of the other worktrees say where git finds their
        // configuration, as the files below do for this one.
        .read_only(&common.join("worktrees"))
        .writable(&own_folder)
        .read_only(&own_config)
        // These say where git finds the git folder of the worktree, and the
        // configuration. So they could name a configuration of the agent.
        .read_only(&folder.join(".git"))
        .read_only(&git::own_file(folder, "commondir")?)
        .read_only(&git::own_file(folder, "gitdir")?);
    Ok(sandbox)
}

/// Makes the file with this content when nothing has its name, also no
/// link. git can read the file at any time, and another run can make it at
/// the same time, so the file is never there with a part of its content:
/// Lira writes another file, and then gives it this name too.
fn make_whole(file: &Path, content: &str) -> std::io::Result<()> {
    if file.symlink_metadata().is_ok() {
        return Ok(());
    }
    // The name is new each time, and Lira makes the file new, so it writes
    // no file that another made, and through no link.
    static MADE: AtomicU64 = AtomicU64::new(0);
    let count = MADE.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let mut name = file.as_os_str().to_owned();
    name.push(format!(".lira-{}-{nanos}-{count}", process::id()));
    let written = Path::new(&name);
    let mut new = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(written)?;
    let made = new
        .write_all(content.as_bytes())
        .and_then(|()| fs::hard_link(written, file));
    let _ = fs::remove_file(written);
    match made {
        Err(error) if error.kind() != std::io::ErrorKind::AlreadyExists => Err(error),
        _ => Ok(()),
    }
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
    /// The sandbox of the agent. The check runs in it too: the check runs
    /// the code of the change, and that is code of the agent.
    sandbox: Sandbox,
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
    fn steps(&mut self) -> Result<String, Stopped> {
        self.work(&message(&self.made.branch, self.request))?;
        let mut tries = 0;
        let mut rounds = 0;
        let mut asked = false;
        loop {
            // Check.
            if let Some(failure) = self.check()? {
                tries += 1;
                if tries == TRIES {
                    return Err(Stopped::Asks {
                        error: format!(
                            "the change did not pass the check {TRIES} times, one after the other, so Lira stops here\nThe last time, {}",
                            failure.for_you
                        ),
                        question: format!(
                            "The agent stops and asks: the change did not pass the check {TRIES} times, one after the other. The work waits for your answer.\n\nThe last time, {}",
                            failure.on_github
                        ),
                    });
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
                // Ready. Lira asks the agent for the description one time.
                // An agent that changes the change when it is asked is not
                // asked again, so this loop ends.
                let description = self.describe(!asked);
                asked = true;
                // The agent can change the worktree while it writes the
                // description. Then the change is not what the check and
                // the Reviewer passed, and both read it again.
                let same = Some(git::head(&self.made.folder)?) == checked
                    && git::changes(&self.made.folder)?.trim().is_empty();
                if !same {
                    self.note("Lira runs the check.")?;
                    continue;
                }
                self.pull_request.ready(&description)?;
                return Ok(self.address()?);
            }
            rounds += 1;
            if rounds == ROUNDS {
                // A comment on GitHub has a largest size, so a long review
                // is cut.
                let text = review.text.trim();
                let start = start_of(text, KEPT);
                let mut quoted: String = start.lines().map(|line| format!("> {line}\n")).collect();
                if start.len() < text.len() {
                    quoted.push_str("\nThe rest is left out.\n");
                }
                return Err(Stopped::Asks {
                    error: format!(
                        "{ROUNDS} rounds of review ended with findings, so Lira stops here\nThe last Reviewer wrote:\n\n{}\n",
                        review.text.trim()
                    ),
                    question: format!(
                        "The agent stops and asks: {ROUNDS} rounds of review ended with findings. The work waits for your answer.\n\nThe last Reviewer wrote:\n\n{quoted}"
                    ),
                });
            }
            self.note("The agent repairs what a Reviewer found.")?;
            self.work(&format!(
                "A Reviewer read your change, and it has findings. Test each finding against the code. Repair the real ones with the smallest change, and commit the repair. For each finding that you do not repair, say why at the end of your reply.\n\nThis is what the Reviewer wrote:\n\n{}\n",
                review.text.trim()
            ))?;
        }
    }

    /// The agent works on a message, and Lira brings the pull request up to
    /// date. After this, all commits of the agent are in the pull request.
    /// When the remote has a branch with the name of the agent already, the
    /// agent gives its branch another name first.
    fn work(&mut self, message: &str) -> Result<(), String> {
        let reply = self.give(message)?;
        self.said = reply.text;
        if let Some(why) = agent::stopped(&reply.stop) {
            return Err(format!(
                "the agent stopped before the end of its work: {why}\n{}",
                wrote(&self.said)
            ));
        }
        let mut asked = 0;
        loop {
            let noted = self.note("Lira runs the check.");
            let Some(name) = self.pull_request.taken().map(str::to_string) else {
                return noted
                    .and_then(|()| self.address())
                    .map(|_| ())
                    .map_err(|error| format!("{error}\n{}", wrote(&self.said)));
            };
            let remote = &self.made.remote;
            if asked == NAMES {
                return Err(format!(
                    "the remote `{remote}` has a branch `{name}` already, and the agent gave its branch no other name after {NAMES} messages, so Lira pushed nothing\n{}",
                    wrote(&self.said)
                ));
            }
            asked += 1;
            eprintln!(
                "lira: the remote `{remote}` has a branch `{name}` already, so the agent gives its branch another name"
            );
            // What the agent wrote at the end of its work stays. Its answer
            // here is only the new name.
            let renamed = self.give(&rename(remote, &name))?;
            if let Some(why) = agent::stopped(&renamed.stop) {
                return Err(format!(
                    "the agent stopped before it gave its branch another name: {why}\n{}",
                    wrote(&self.said)
                ));
            }
        }
    }

    /// Gives a message to the agent, and waits for its reply. While the
    /// agent works, Lira looks for a new commit each time a tool call ends.
    fn give(&mut self, message: &str) -> Result<Reply, String> {
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
        reply.map_err(|error| match self.pull_request.problem() {
            Some(problem) => format!("{error}\n{problem}"),
            None => error.to_string(),
        })
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

    /// The description for the pull request that is ready for review. With
    /// `ask`, the agent writes it. When the agent does not, or writes it
    /// with other headings, Lira writes it from what it has.
    fn describe(&mut self, ask: bool) -> String {
        let written = ask.then(|| {
            // The agent has no reason to change a file here, so the answer
            // to each permission request is no.
            self.harness
                .send(&self.conversation, DESCRIBE, &mut |_| false)
        });
        let written = match &written {
            Some(Ok(reply)) if agent::stopped(&reply.stop).is_none() => description_in(&reply.text),
            _ => None,
        };
        match written {
            Some(written) => written.to_string(),
            None => {
                let from = &self.made.commit;
                let subjects = git::subjects(&self.made.folder, from, "HEAD").unwrap_or_default();
                own_description(self.request, &subjects, &self.said)
            }
        }
    }

    /// Says in the progress note that Lira stopped, and why. GitHub is
    /// where the person reads how far the work is. When the agent stops and
    /// asks, Lira also writes the question as a comment, and adds
    /// `needs:answer`. It returns the error of the run, with what went
    /// wrong here. The run failed already, so a failure of the progress
    /// note changes nothing.
    fn stop(&mut self, stopped: Stopped) -> String {
        let (error, question) = match stopped {
            Stopped::Failed(error) => (error, None),
            Stopped::Asks { error, question } => (error, Some(question)),
        };
        if self.pull_request.address().is_none() {
            return error;
        }
        let why = error.lines().next().unwrap_or_default();
        let _ = self.note(&format!(
            "Nothing. Lira stopped, and the pull request waits for you: {why}."
        ));
        match question.map(|question| self.pull_request.ask(&question)) {
            Some(Err(problem)) => format!("{error}\n{problem}"),
            _ => error,
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
                on_github: format!(
                    "the worktree had changes that are in no commit, so the check did not run:\n\n{}",
                    fenced(changes)
                ),
            }));
        }
        let main = format!("refs/remotes/{}/{}", self.made.remote, self.made.main);
        let base = git::base(folder, &main)?;
        let outcome = check::run(
            &self.made.file,
            folder,
            &base,
            Output::Kept,
            Some(&self.sandbox),
        )?;
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
                on_github: format!(
                    "the check, `{command}`, passed, and it left changes that are in no commit:\n\n{}",
                    fenced(changes)
                ),
            }));
        }
        let status = &outcome.status;
        let end = end_of(&outcome.wrote, KEPT);
        // The comment on GitHub names no file of the host.
        let on_github = match end {
            "" => "The check wrote nothing.\n".to_string(),
            end if end.len() < outcome.wrote.trim_end().len() => format!(
                "This is the end of what the check wrote. The start is left out.\n\n{}",
                fenced(end)
            ),
            end => format!("This is what the check wrote:\n\n{}", fenced(end)),
        };
        let wrote = match end {
            "" => "The check wrote nothing.\n".to_string(),
            end if end.len() < outcome.wrote.trim_end().len() => {
                // The start of a long output can say what is wrong. The
                // agent reads all of it in a file that git keeps for the
                // worktree, outside the files of the repository.
                self.outputs += 1;
                let name = format!("lira-check-output-{}.txt", self.outputs);
                let whole = git::own_file(folder, &name).and_then(|file| {
                    git::write_new(&file, &outcome.wrote)
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
            on_github: format!("the check, `{command}`, ended with {status}. {on_github}"),
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
            on_github: format!(
                "the worktree was not at the newest commit of the branch `{branch}` of the agent {you}.\n"
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

/// The description in a reply of the agent: the text from the line
/// `## Problem` on. It is `None` when the text does not have the three
/// headings in their order.
fn description_in(text: &str) -> Option<&str> {
    let mut wanted = ["## Problem", "## Change", "## Decisions"]
        .into_iter()
        .peekable();
    let mut start = None;
    let mut at = 0;
    // A line in a block of code is no heading. `code` is the row of
    // backticks or tildes that opened the block that the line is in.
    let mut code: Option<(u8, usize)> = None;
    for line in text.split_inclusive('\n') {
        match code {
            Some(open) if closes_code(line, open) => code = None,
            Some(_) => {}
            None => match opens_code(line) {
                Some(open) => code = Some(open),
                None => {
                    if wanted
                        .peek()
                        .is_some_and(|heading| line.trim_end() == *heading)
                    {
                        wanted.next();
                        start = start.or(Some(at));
                    }
                }
            },
        }
        at += line.len();
    }
    match (start, wanted.peek()) {
        (Some(start), None) => Some(text[start..].trim_end()),
        _ => None,
    }
}

/// If a line opens a block of code: it starts with three or more backticks,
/// or three or more tildes, after at most three spaces. It returns the
/// character and how many there are.
fn opens_code(line: &str) -> Option<(u8, usize)> {
    let line = row_of_code(line)?;
    let byte = *line.as_bytes().first()?;
    if byte != b'`' && byte != b'~' {
        return None;
    }
    let length = line.bytes().take_while(|b| *b == byte).count();
    (length >= 3).then_some((byte, length))
}

/// If a line closes the block of code that `open` opened: it has the same
/// character, at least as many times, and nothing after it.
fn closes_code(line: &str, (byte, length): (u8, usize)) -> bool {
    let Some(line) = row_of_code(line) else {
        return false;
    };
    let run = line.bytes().take_while(|b| *b == byte).count();
    run >= length && line[run..].trim().is_empty()
}

/// A line with no spaces at its start, when it has at most three there. A
/// line with more is text of a block, and no start or end of one.
fn row_of_code(line: &str) -> Option<&str> {
    let spaces = line.bytes().take_while(|b| *b == b' ').count();
    (spaces <= 3).then(|| &line[spaces..])
}

/// The description that Lira writes when the agent wrote none: the request,
/// the subject of each commit, and what the agent said at the end of its
/// work.
fn own_description(request: &str, subjects: &[String], said: &str) -> String {
    let request: String = request.lines().map(|line| format!("> {line}\n")).collect();
    let change: String = subjects
        .iter()
        .map(|subject| format!("- {subject}\n"))
        .collect();
    // What the agent said is a quote, as the request is. So a heading in
    // it is no heading of the description.
    let decisions = match said.trim() {
        "" => "The agent named none.\n".to_string(),
        said => {
            let said: String = said.lines().map(|line| format!("> {line}\n")).collect();
            format!("The agent wrote this at the end of its work:\n\n{said}")
        }
    };
    format!(
        "## Problem\n\nThe request was:\n\n{request}\n## Change\n\n{change}\n## Decisions\n\n{decisions}"
    )
}

/// Why the steps ended before the pull request was ready for review.
enum Stopped {
    /// Something went wrong. The text is the error.
    Failed(String),
    /// The agent stops and asks. `error` is the error of the run, and
    /// `question` is the comment that says what stops the work.
    Asks { error: String, question: String },
}

impl From<String> for Stopped {
    fn from(error: String) -> Stopped {
        Stopped::Failed(error)
    }
}

/// A text in a block of code, whose fence is longer than each row of
/// backticks in the text.
fn fenced(text: &str) -> String {
    let mut longest = 0;
    let mut row = 0;
    for c in text.chars() {
        row = if c == '`' { row + 1 } else { 0 };
        longest = longest.max(row);
    }
    let fence = "`".repeat(longest.max(2) + 1);
    format!("{fence}\n{}\n{fence}\n", text.trim_end())
}

/// A check that did not pass.
struct Failure {
    /// The message that tells the agent to repair the change.
    for_the_agent: String,
    /// The end of the error for the person, when Lira stops.
    for_you: String,
    /// The end of the comment on the pull request, when Lira stops. It is
    /// Markdown, and it names no file of the host.
    on_github: String,
}

/// The message for an agent whose branch has a name that the remote has
/// already.
fn rename(remote: &str, name: &str) -> String {
    format!(
        "The remote `{remote}` has a branch `{name}` already, and it is not your work. So Lira can't push your branch with this name. Give your branch another name that says what the work is: run `git branch -m <name>`. Change nothing else, and answer with the new name only.\n"
    )
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

/// The start of a text: at most `most` bytes, cut between two characters.
fn start_of(text: &str, most: usize) -> &str {
    let mut end = most.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
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
    fn a_description_has_the_three_headings_in_their_order() {
        let good = "## Problem\n\nP.\n\n## Change\n\nC.\n\n### More\n\n## Decisions\n\nNone.";
        assert_eq!(description_in(good), Some(good));
        // Text before the first heading is left out, and spaces at the end.
        assert_eq!(
            description_in(&format!("Here it is:\n\n{good}  \n\n")),
            Some(good)
        );
        assert_eq!(
            description_in("## Problem\n## Decisions\n## Change\n"),
            None
        );
        assert_eq!(description_in("## Problem\n## Change\n"), None);
        assert_eq!(description_in("Problem, Change, Decisions"), None);
        assert_eq!(description_in(""), None);
        // A line in a block of code is no heading.
        let quoted = "## Problem\n\nThe old form was:\n\n```\n## Change\n## Decisions\n```\n";
        assert_eq!(description_in(quoted), None);
        // A shorter row of backticks does not end a block, and a row with
        // text after it does not end one.
        let longer = "## Problem\n````\n```\n## Change\n## Decisions\n````\n";
        assert_eq!(description_in(longer), None);
        let not_an_end = "## Problem\n```\n``` sh\n## Change\n## Decisions\n```\n";
        assert_eq!(description_in(not_an_end), None);
        let never_ends = "## Problem\n```\n## Change\n## Decisions\n";
        assert_eq!(description_in(never_ends), None);
        let with_code =
            "## Problem\n\n~~~sh\n## a comment\n~~~\n\n## Change\n\nC.\n\n## Decisions\n\nNone.";
        assert_eq!(description_in(with_code), Some(with_code));
    }

    #[test]
    fn lira_writes_a_description_from_what_it_has() {
        let subjects = ["Add a file".to_string()];
        assert_eq!(
            own_description("Add a file.\nMake it short.", &subjects, " Done. \n"),
            "## Problem\n\nThe request was:\n\n> Add a file.\n> Make it short.\n\n## Change\n\n- Add a file\n\n## Decisions\n\nThe agent wrote this at the end of its work:\n\n> Done.\n"
        );
        // A heading in what the agent said is no heading of the description.
        let with_heading = own_description("Do it.", &subjects, "Done.\n\n## Summary\n\nA file.");
        let headings: Vec<&str> = with_heading
            .lines()
            .filter(|line| line.starts_with('#'))
            .collect();
        assert_eq!(headings, ["## Problem", "## Change", "## Decisions"]);
        assert!(with_heading.contains("> ## Summary\n"));
        assert!(own_description("Do it.", &subjects, "").ends_with("\n\nThe agent named none.\n"));
    }

    #[test]
    fn the_start_of_a_text_is_cut_between_two_characters() {
        assert_eq!(start_of("abc", 10), "abc");
        assert_eq!(start_of("abc", 2), "ab");
        assert_eq!(start_of("éé", 3), "é");
    }

    #[test]
    fn a_fence_is_longer_than_each_row_of_backticks_in_the_text() {
        assert_eq!(fenced("one\ntwo\n"), "```\none\ntwo\n```\n");
        assert_eq!(fenced("a ```` b"), "`````\na ```` b\n`````\n");
    }

    #[test]
    fn a_new_branch_has_a_new_name() {
        let (one, other) = (new_branch(), new_branch());
        assert_ne!(one, other);
        assert_eq!(one.len(), "lira/run-".len() + 8);
    }

    #[test]
    fn lira_makes_a_file_whole_and_leaves_what_has_its_name() {
        let folder = std::env::temp_dir().join(format!("lira-make-whole-{}", process::id()));
        let _ = fs::remove_dir_all(&folder);
        fs::create_dir_all(&folder).unwrap();
        let file = folder.join("commondir");
        make_whole(&file, ".\n").unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), ".\n");
        make_whole(&file, "other\n").unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), ".\n");
        // Lira writes nothing through a link, also when its target is not
        // there.
        let link = folder.join("config.worktree");
        std::os::unix::fs::symlink(folder.join("elsewhere"), &link).unwrap();
        make_whole(&link, "").unwrap();
        assert!(!folder.join("elsewhere").exists());
        // Lira leaves no other file.
        assert_eq!(fs::read_dir(&folder).unwrap().count(), 2);
        fs::remove_dir_all(&folder).unwrap();
    }
}
