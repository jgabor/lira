//! `lira run`: an agent works on one request, in a new worktree.

use crate::harness::{Harness, Stop};
use crate::json::Json;
use crate::worktree::{self, Worktree};
use crate::{data, git};
use std::ffi::OsString;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::process::{self, Command};
use std::time::{SystemTime, UNIX_EPOCH};

/// The instructions for the agent. The request follows them.
const INSTRUCTIONS: &str = include_str!("instructions.md");

/// The program of the harness when `LIRA_HARNESS` names none. It is the
/// program that lets Claude Code speak the Agent Client Protocol.
const HARNESS: &str = "claude-agent-acp";

/// `lira run "<request>"`: makes a new worktree of the repository in
/// `folder`, and runs an agent on the request in it. It returns when the
/// reply of the agent ends.
pub fn command(folder: &Path, request: &str) -> Result<(), String> {
    let data = data::folder()?;
    let made = worktree::make(folder, &data, &new_branch())?;
    let place = format!("`{}`", made.folder.display());
    eprintln!("lira: the agent works in {place}");

    let (mut harness, conversation) = match start(&made) {
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

    // Nobody is there to answer, and the agent works in a worktree of its
    // own. So the answer to each permission request is yes.
    let reply = harness.send(&conversation, &message(&made.branch, request), &mut |_| {
        true
    });
    // The harness stops here, so that the agent changes nothing after this.
    drop(harness);
    let reply = reply.map_err(|error| format!("{error}\nWhat the agent did is in {place}"))?;
    let said = match reply.text.trim() {
        "" => "The agent wrote nothing".to_string(),
        text => format!("The agent wrote:\n\n{text}\n"),
    };
    if let Some(why) = stopped(&reply.stop) {
        return Err(format!(
            "the agent stopped before the end of its work: {why}. What it did is in {place}\n{said}"
        ));
    }
    match git::current_branch(&made.folder)? {
        Some(branch) => {
            eprintln!("lira: the work of the agent is on the branch `{branch}`, in {place}")
        }
        None => eprintln!("lira: the work of the agent is in {place}, on no branch"),
    }
    eprintln!("{said}");
    Ok(())
}

/// Starts the harness in the worktree, and a conversation with the agent.
fn start(made: &Worktree) -> Result<(Harness, String), String> {
    let named = std::env::var_os("LIRA_HARNESS").filter(|program| !program.is_empty());
    let program = named.unwrap_or_else(|| OsString::from(HARNESS));
    let mut command = Command::new(&program);
    command.current_dir(&made.folder);
    let mut harness = Harness::start(command).map_err(|error| {
        format!("{error}. `LIRA_HARNESS` names the program of the harness, and with no value it is `{HARNESS}`")
    })?;
    // The protocol has no place for the AI model. This is where the harness
    // of Claude Code reads it.
    let model = made.file.models.agent.as_deref().map(|model| {
        let options = Json::object([("model", Json::text(model))]);
        Json::object([("claudeCode", Json::object([("options", options)]))])
    });
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
