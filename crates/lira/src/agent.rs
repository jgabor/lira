//! An agent: a harness that runs, and one conversation with it. The agent
//! that does the work and each Reviewer start in the same way.

use crate::harness::{Harness, Stop};
use crate::lira_file::LiraFile;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The program of the harness when `LIRA_HARNESS` names none, and the Lira
/// file names none. It is the program that lets Claude Code speak the Agent
/// Client Protocol.
const HARNESS: &str = "claude-agent-acp";

/// Starts the harness in `folder`, and a conversation with an agent that
/// works there. It returns the harness and the name of the conversation.
/// `from` is the folder that Lira started in. `file` is the Lira file, and
/// `model` is the AI model for the agent, when the Lira file names one.
pub fn start(
    from: &Path,
    folder: &Path,
    file: &LiraFile,
    model: Option<&str>,
) -> Result<(Harness, String), String> {
    let named = std::env::var_os("LIRA_HARNESS").filter(|program| !program.is_empty());
    let program = match named {
        Some(named) => {
            let program = PathBuf::from(named);
            // A name with no folder is a program of `PATH`. A path starts
            // at the folder that Lira started in, and not at the folder
            // where the harness starts.
            let is_path = program.components().count() > 1;
            if is_path { from.join(program) } else { program }
        }
        // The Lira file has a name, or a path from the root of the host.
        None => PathBuf::from(file.harness.program.as_deref().unwrap_or(HARNESS)),
    };
    let mut command = Command::new(&program);
    command.current_dir(folder);
    let mut harness = Harness::start(command).map_err(|error| {
        format!(
            "{error}. `LIRA_HARNESS` names the program of the harness. With no value, it is `harness.program` in `lira.toml`, and with no such key it is `{HARNESS}`"
        )
    })?;
    let conversation = harness
        .converse(folder, model)
        .map_err(|error| error.to_string())?;
    Ok((harness, conversation))
}

/// Why an agent did not end its reply, or `None` when it did.
pub fn stopped(stop: &Stop) -> Option<String> {
    Some(match stop {
        Stop::End => return None,
        Stop::MaxTokens => "the AI model reached its limit of tokens".to_string(),
        Stop::MaxRequests => "the agent reached its limit of requests to the AI model".to_string(),
        Stop::Refusal => "the AI model refused to continue".to_string(),
        Stop::Cancelled => "the message was stopped".to_string(),
        Stop::Other(reason) => format!("the harness gave the reason `{reason}`"),
    })
}
