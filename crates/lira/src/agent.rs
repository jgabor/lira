//! An agent: a harness that runs, and one conversation with it. The agent
//! that does the work and each Reviewer start in the same way.

use crate::harness::{Harness, Stop};
use crate::lira_file::LiraFile;
use crate::sandbox::Sandbox;
use std::path::{Path, PathBuf};

/// The program of the harness when `LIRA_HARNESS` names none, and the Lira
/// file names none. It is the program that lets Claude Code speak the Agent
/// Client Protocol.
const HARNESS: &str = "claude-agent-acp";

/// Starts the harness in `folder`, and a conversation with an agent that
/// works there. It returns the harness and the name of the conversation.
/// `from` is the folder that Lira started in. `file` is the Lira file, and
/// `model` is the AI model for the agent, when the Lira file names one.
///
/// The harness runs in `sandbox`. It can change its own data there too, and
/// `sandbox` has that data afterwards: it is then the sandbox of the agent.
pub fn start(
    from: &Path,
    folder: &Path,
    sandbox: &mut Sandbox,
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
    let named = |error: String| {
        format!(
            "{error}. `LIRA_HARNESS` names the program of the harness. With no value, it is `harness.program` in `lira.toml`, and with no such key it is `{HARNESS}`"
        )
    };
    let shown = program.display().to_string();
    let found = find(&program).ok_or_else(|| {
        named(format!(
            "Lira can't find the harness `{shown}` on this host, or a program that it needs to start"
        ))
    })?;
    for data in harness_data() {
        sandbox.writable(&data);
        // What tells the harness how to work, and what it runs, stays as it
        // is. The harness reads it in each later conversation, also outside
        // a sandbox.
        for (name, empty) in OWN_SETTINGS {
            let path = data.join(name);
            // The agent could make what is not there. So Lira makes it
            // empty first, and that changes nothing for the harness.
            if path.symlink_metadata().is_err() {
                let made = match empty {
                    Some(text) => std::fs::write(&path, text),
                    None => std::fs::create_dir(&path),
                };
                made.map_err(|error| format!("Lira can't make `{}`: {error}", path.display()))?;
            }
            // A link can't be read-only in its folder. The file that it
            // names can.
            if let Ok(path) = path.canonicalize() {
                sandbox.read_only(&path);
            }
        }
    }
    let mut command = sandbox.command(found.as_os_str());
    command.current_dir(folder);
    let mut harness =
        Harness::start_as(command, &shown).map_err(|error| named(error.to_string()))?;
    let conversation = harness
        .converse(folder, model)
        .map_err(|error| error.to_string())?;
    Ok((harness, conversation))
}

/// The file of a program: the path itself, or for a name with no folder,
/// the first file with that name in a folder of `PATH`.
fn find(program: &Path) -> Option<PathBuf> {
    if program.components().count() > 1 {
        return program.is_file().then(|| program.to_path_buf());
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|folder| folder.join(program))
        .find(|file| file.is_file())
}

/// What the harness keeps in its data folder that says how it works, and
/// what it runs: its instructions, its settings with their hooks, and its
/// plugins.
/// Each is a file with its text when it is empty, or a folder.
const OWN_SETTINGS: [(&str, Option<&str>); 9] = [
    ("CLAUDE.md", Some("")),
    ("settings.json", Some("{}\n")),
    ("settings.local.json", Some("{}\n")),
    ("hooks", None),
    ("agents", None),
    ("commands", None),
    ("skills", None),
    ("plugins", None),
    ("output-styles", None),
];

/// The folders where the harness keeps its own data, such as its login and
/// its conversations. The harness can change them in its sandbox. Claude
/// Code keeps its data in `CLAUDE_CONFIG_DIR`, or in `~/.claude` when that
/// is not set. A folder that is not there is left out.
fn harness_data() -> Vec<PathBuf> {
    let absolute = |name: &str| {
        std::env::var_os(name)
            .map(PathBuf::from)
            .filter(|folder| folder.is_absolute())
    };
    let data =
        absolute("CLAUDE_CONFIG_DIR").or_else(|| absolute("HOME").map(|home| home.join(".claude")));
    data.into_iter().filter(|folder| folder.is_dir()).collect()
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
