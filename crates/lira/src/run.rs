//! `lira run`: an agent works on one request, in a new worktree, and Lira
//! opens the pull request for its work.

use crate::harness::{self, Harness, Reply, Stop};
use crate::pull_request::PullRequest;
use crate::worktree::{self, Worktree};
use crate::{data, github};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{self, Command};
use std::sync::mpsc;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

/// The instructions for the agent. The request follows them.
const INSTRUCTIONS: &str = include_str!("instructions.md");

/// The program of the harness when `LIRA_HARNESS` names none, and the Lira
/// file names none. It is the program that lets Claude Code speak the Agent
/// Client Protocol.
const HARNESS: &str = "claude-agent-acp";

/// How many times Lira tells the agent to give its branch another name,
/// when the remote has a branch with the name of the agent already.
const NAMES: usize = 3;

/// `lira run "<request>"`: makes a new worktree of the repository in
/// `folder`, and runs an agent on the request in it. It opens a draft pull
/// request at the first commit of the agent, and prints the address of the
/// pull request when the reply of the agent ends.
pub fn command(folder: &Path, request: &str) -> Result<(), String> {
    // A missing login stops the run here, before an agent works.
    github::require_login(folder)?;
    let data = data::folder()?;
    let made = worktree::make(folder, &data, &new_branch())?;
    let place = format!("`{}`", made.folder.display());
    eprintln!("lira: the agent works in {place}");

    let (harness, conversation) = match start(folder, &made) {
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

    let mut harness = harness;
    let mut pull_request = PullRequest::new(&made)?;
    let worked = work(
        &mut harness,
        &conversation,
        &made,
        request,
        &mut pull_request,
    );
    // The harness stops here, so that the agent changes nothing after this.
    drop(harness);
    // A process of the harness can make a commit until the harness stops.
    // So Lira looks one more time, and the pull request has all the work.
    let worked = worked.and_then(|text| match pull_request.complete(&text) {
        Ok(address) => Ok((address, text)),
        Err(error) => Err(Failure {
            error,
            said: Some(text),
        }),
    });
    let (address, text) = match worked {
        Ok(worked) => worked,
        Err(failure) => {
            // Where the work is.
            let mut left = format!("What the agent did is in {place}");
            if let Some(branch) = pull_request.branch() {
                left.push_str(&format!(
                    ", and on the branch `{branch}` of the remote `{}`",
                    made.remote
                ));
            }
            if let Some(address) = pull_request.address() {
                left.push_str(&format!("\nThe pull request is a draft: {address}"));
            }
            let said = failure
                .said
                .map_or(String::new(), |text| format!("\n{}", wrote(&text)));
            return Err(format!("{}\n{left}{said}", failure.error));
        }
    };
    let branch = pull_request.branch().unwrap_or(&made.branch);
    eprintln!(
        "lira: the work of the agent is complete. It is on the branch `{branch}`, and its worktree is {place}"
    );
    eprintln!("{}", wrote(&text));
    println!("{address}");
    Ok(())
}

/// What went wrong after the agent started. `said` is what the agent wrote
/// at the end of its work, when its reply ended.
struct Failure {
    error: String,
    said: Option<String>,
}

/// What the agent wrote, for the output of `lira run`.
fn wrote(text: &str) -> String {
    match text.trim() {
        "" => "The agent wrote nothing".to_string(),
        text => format!("The agent wrote:\n\n{text}\n"),
    }
}

/// The agent works on the request, and Lira brings the pull request up to
/// date. When the remote has a branch with the name of the agent already,
/// the agent gives its branch another name. It returns what the agent wrote
/// at the end of its work.
fn work(
    harness: &mut Harness,
    conversation: &str,
    made: &Worktree,
    request: &str,
    pull_request: &mut PullRequest,
) -> Result<String, Failure> {
    let first = message(&made.branch, request);
    let reply = give(harness, conversation, &first, pull_request).map_err(|error| Failure {
        error: match pull_request.problem() {
            Some(problem) => format!("{error}\n{problem}"),
            None => error.to_string(),
        },
        said: None,
    })?;
    let fail = |error: String| Failure {
        error,
        said: Some(reply.text.clone()),
    };
    if let Some(why) = stopped(&reply.stop) {
        return Err(fail(format!(
            "the agent stopped before the end of its work: {why}"
        )));
    }
    let remote = &made.remote;
    let mut asked = 0;
    loop {
        let complete = pull_request.complete(&reply.text);
        let Some(name) = pull_request.taken().map(str::to_string) else {
            return complete.map(|_| reply.text.clone()).map_err(fail);
        };
        if asked == NAMES {
            return Err(fail(format!(
                "the remote `{remote}` has a branch `{name}` already, and the agent gave its branch no other name after {NAMES} messages, so Lira pushed nothing"
            )));
        }
        asked += 1;
        eprintln!(
            "lira: the remote `{remote}` has a branch `{name}` already, so the agent gives its branch another name"
        );
        let renamed = give(harness, conversation, &rename(remote, &name), pull_request)
            .map_err(|error| fail(error.to_string()))?;
        if let Some(why) = stopped(&renamed.stop) {
            return Err(fail(format!(
                "the agent stopped before it gave its branch another name: {why}"
            )));
        }
    }
}

/// The message for an agent whose branch has a name that the remote has
/// already.
fn rename(remote: &str, name: &str) -> String {
    format!(
        "The remote `{remote}` has a branch `{name}` already, and it is not your work. So Lira can't push your branch with this name. Give your branch another name that says what the work is: run `git branch -m <name>`. Change nothing else, and answer with the new name only.\n"
    )
}

/// Gives a message to the agent, and waits for its reply. While the agent
/// works, Lira looks for a new commit each time a tool call ends.
fn give(
    harness: &mut Harness,
    conversation: &str,
    message: &str,
    pull_request: &mut PullRequest,
) -> Result<Reply, harness::Error> {
    // Lira looks for a new commit on a thread of its own. So the agent does
    // not wait for a permission while Lira talks to the remote or to
    // GitHub.
    thread::scope(|scope| {
        let (look, looks) = mpsc::channel();
        scope.spawn(move || {
            while looks.recv().is_ok() {
                // One look is enough for all tool calls that ended since
                // the look before.
                while looks.try_recv().is_ok() {}
                pull_request.look();
            }
        });
        harness.send_and_watch(
            conversation,
            message,
            // Nobody is there to answer, and the agent works in a worktree
            // of its own. So the answer to each permission request is yes.
            &mut |_| true,
            // A tool call can be the first commit, or a later one.
            &mut || {
                let _ = look.send(());
            },
        )
        // The thread ends here: nothing can ask for a look any more.
    })
}

/// Starts the harness in the worktree, and a conversation with the agent.
/// `folder` is the folder that `lira run` started in.
fn start(folder: &Path, made: &Worktree) -> Result<(Harness, String), String> {
    let named = std::env::var_os("LIRA_HARNESS").filter(|program| !program.is_empty());
    let program = match named {
        Some(named) => {
            let program = PathBuf::from(named);
            // A name with no folder is a program of `PATH`. A path starts
            // at the folder that `lira run` started in, and not at the
            // worktree, where the harness starts.
            let is_path = program.components().count() > 1;
            if is_path {
                folder.join(program)
            } else {
                program
            }
        }
        // The Lira file has a name, or a path from the root of the host.
        None => PathBuf::from(made.file.harness.program.as_deref().unwrap_or(HARNESS)),
    };
    let mut command = Command::new(&program);
    command.current_dir(&made.folder);
    let mut harness = Harness::start(command).map_err(|error| {
        format!(
            "{error}. `LIRA_HARNESS` names the program of the harness. With no value, it is `harness.program` in `lira.toml`, and with no such key it is `{HARNESS}`"
        )
    })?;
    let model = made.file.models.agent.as_deref();
    let conversation = harness
        .converse(&made.folder, model)
        .map_err(|error| error.to_string())?;
    Ok((harness, conversation))
}

/// The message for the agent: the instructions, then the request as it is.
fn message(branch: &str, request: &str) -> String {
    format!("{}{request}", INSTRUCTIONS.replace("{branch}", branch))
}

/// Why the agent did not end its work, or `None` when it did.
fn stopped(stop: &Stop) -> Option<String> {
    Some(match stop {
        Stop::End => return None,
        Stop::MaxTokens => "the AI model reached its limit of tokens".to_string(),
        Stop::MaxRequests => "the agent reached its limit of requests to the AI model".to_string(),
        Stop::Refusal => "the AI model refused to continue".to_string(),
        Stop::Cancelled => "the message was stopped".to_string(),
        Stop::Other(reason) => format!("the harness gave the reason `{reason}`"),
    })
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
    fn a_new_branch_has_a_new_name() {
        let (one, other) = (new_branch(), new_branch());
        assert_ne!(one, other);
        assert_eq!(one.len(), "lira/run-".len() + 8);
    }
}
