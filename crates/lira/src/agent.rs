//! An agent: a harness that runs, and one conversation with it. The agent
//! that does the work and each Reviewer start in the same way.

use crate::data;
use crate::harness::{Harness, Stop};
use crate::lira_file::LiraFile;
use crate::sandbox::Sandbox;
use std::fs::{self, DirBuilder, File, TryLockError};
use std::io::ErrorKind;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// The program of the harness when `LIRA_HARNESS` names none, and the Lira
/// file names none. It is the program that lets Claude Code speak the Agent
/// Client Protocol.
const HARNESS: &str = "claude-agent-acp";

/// An agent that runs: its harness, and its conversation with it.
pub struct Agent {
    pub harness: Harness,
    /// The name of the conversation.
    pub conversation: String,
    /// The data folder of the harness. It comes after the harness, so that
    /// it goes away after the harness ended.
    _data: Data,
}

/// Starts the harness in `folder`, and a conversation with an agent that
/// works there. `from` is the folder that Lira started in. `file` is the
/// Lira file, and `model` is the AI model for the agent, when the Lira file
/// names one.
///
/// The harness runs in `sandbox`, with a data folder of its own, as `Data`
/// says. `sandbox` has that data afterwards: it is then the sandbox of the
/// agent.
pub fn start(
    from: &Path,
    folder: &Path,
    sandbox: &mut Sandbox,
    file: &LiraFile,
    model: Option<&str>,
) -> Result<Agent, String> {
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
    // The harness gets the token in its environment. In the sandbox, the
    // file of the token is empty, also in the place that Lira does not
    // read. The commands that the harness runs get the environment too.
    for file in token_files() {
        sandbox.hidden(&file);
    }
    let token = match token_file() {
        Some(file) => token(&file)?,
        None => None,
    };
    let data = Data::new()?;
    data.prepare(sandbox, token.is_none())?;
    let mut command = sandbox.command(found.as_os_str());
    command.current_dir(folder).env(DATA_FOLDER, &data.folder);
    if let Some(token) = token {
        command.env(TOKEN, token);
    }
    let mut harness =
        Harness::start_as(command, &shown).map_err(|error| named(error.to_string()))?;
    let conversation = harness
        .converse(folder, model)
        .map_err(|error| error.to_string())?;
    Ok(Agent {
        harness,
        conversation,
        _data: data,
    })
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

/// The variable that names the data folder of Claude Code. With no value,
/// it is `~/.claude`.
const DATA_FOLDER: &str = "CLAUDE_CONFIG_DIR";

/// What the harness keeps in its data folder that says how it works, and
/// what it runs: its instructions, its settings with their hooks, and its
/// plugins.
const OWN_SETTINGS: [&str; 11] = [
    "CLAUDE.md",
    "rules",
    "settings.json",
    "settings.local.json",
    "hooks",
    "agents",
    "commands",
    "skills",
    "workflows",
    "plugins",
    "output-styles",
];

/// The file of the login of the harness, in its data folder.
const LOGIN: &str = ".credentials.json";

/// The variable where Claude Code reads a login token. The harness does
/// not renew such a token.
const TOKEN: &str = "CLAUDE_CODE_OAUTH_TOKEN";

/// The file of the login token, in the configuration folder of the host.
const TOKEN_FILE: [&str; 3] = ["lira", "auth", "claude-oauth-token"];

/// The file of the login token for the harness. It is in
/// `XDG_CONFIG_HOME`, or in `~/.config` when that is not set. A value that
/// is empty, or that is no absolute path, counts as not set, as for
/// `XDG_DATA_HOME`.
fn token_file() -> Option<PathBuf> {
    let folder = absolute("XDG_CONFIG_HOME")
        .or_else(|| absolute("HOME").map(|home| home.join(".config")))?;
    Some(TOKEN_FILE.iter().fold(folder, |path, name| path.join(name)))
}

/// The places of the file of the login token: in `XDG_CONFIG_HOME`, and in
/// `~/.config`.
fn token_files() -> Vec<PathBuf> {
    let folders = [
        absolute("XDG_CONFIG_HOME"),
        absolute("HOME").map(|home| home.join(".config")),
    ];
    let files = folders.into_iter().flatten();
    files
        .map(|folder| TOKEN_FILE.iter().fold(folder, |path, name| path.join(name)))
        .collect()
}

/// The login token for the harness in `file`, or `None` when the file is
/// not there.
///
/// `claude setup-token` makes the token. The harness does not renew it, so
/// it keeps working when Claude Code on the host renews its own login.
/// Lira gives the token to the harness in its environment, and writes it
/// nowhere.
fn token(file: &Path) -> Result<Option<String>, String> {
    let text = match fs::read_to_string(file) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "Lira can't read the login token of the harness in `{}`: {error}",
                file.display()
            ));
        }
    };
    // A token is one word. A file with two words, or two lines, holds no
    // token that the harness can use.
    match text.trim() {
        token if !token.is_empty() && !token.contains(char::is_whitespace) => {
            Ok(Some(token.to_string()))
        }
        _ => Err(format!(
            "the file `{}` holds no login token for the harness. Write only the token that `claude setup-token` prints there, or remove the file",
            file.display()
        )),
    }
}

/// The configuration of Claude Code. It is in the folder that
/// `CLAUDE_CONFIG_DIR` names, or in the home folder.
const CONFIGURATION: &str = ".claude.json";

/// The number of the next data folder that this process makes.
static NEXT: AtomicUsize = AtomicUsize::new(0);

/// The data folder of the harness of one agent, such as for its
/// conversations. Each agent has a new one, in the folder `harness` of the
/// data folder of Lira, and Lira removes it when the agent ends.
///
/// The process of Lira that made the folder holds a lock on it until the
/// folder is gone. A process that ends with no time to remove the folder,
/// as after `SIGKILL`, loses the lock all the same. So a folder with no lock
/// is one that Lira left, and the next agent removes it.
///
/// The harness reads its data folder in each later conversation, also
/// outside a sandbox. So in the sandbox, the data folder of the host is
/// read-only, with all that is in it, and the harness has this one. Lira
/// puts these in it, from the data folder of the host:
///
/// - What says how the harness works, read-only.
/// - The login, which the harness can change, when Lira has no login token
///   for the harness. It renews the login, and a login that it can't save
///   could log you out.
/// - A copy of the configuration of Claude Code, which the harness can
///   change, and the copy goes away with the folder.
struct Data {
    folder: PathBuf,
    /// The open folder, with the lock on it. `drop` removes the folder
    /// before the lock goes away.
    _lock: File,
}

impl Data {
    /// Makes a new data folder, which only you can read.
    fn new() -> Result<Data, String> {
        Data::new_in(&data::folder()?.join("harness"))
    }

    /// Makes a new data folder in `above`. First it removes each folder
    /// there that Lira left.
    fn new_in(above: &Path) -> Result<Data, String> {
        let failed = |folder: &Path, error| {
            format!("Lira can't make the folder `{}`: {error}", folder.display())
        };
        fs::create_dir_all(above).map_err(|error| failed(above, error))?;
        remove_left(above);
        loop {
            let name = format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            );
            let folder = above.join(name);
            // The folder must be new: Lira removes it with all that is in it.
            match DirBuilder::new().mode(0o700).create(&folder) {
                Ok(()) => {}
                // A folder of an earlier process with the same number.
                Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(failed(&folder, error)),
            }
            // Another process of Lira can find the folder before the lock
            // is on it, and remove it. Then Lira makes another one.
            let lock = match File::open(&folder) {
                Ok(lock) => lock,
                Err(error) if error.kind() == ErrorKind::NotFound => continue,
                Err(error) => return Err(failed(&folder, error)),
            };
            match lock.try_lock() {
                Ok(()) if folder.is_dir() => {
                    return Ok(Data {
                        folder,
                        _lock: lock,
                    });
                }
                Ok(()) | Err(TryLockError::WouldBlock) => continue,
                Err(TryLockError::Error(error)) => return Err(failed(&folder, error)),
            }
        }
    }

    /// Puts in the data folder what the harness needs from the data folder
    /// of the host, and makes it writable in `sandbox`. The login of the
    /// host is there only with `login`. Without it, the login of the host
    /// is empty in `sandbox`.
    fn prepare(&self, sandbox: &mut Sandbox, login: bool) -> Result<(), String> {
        sandbox.writable(&self.folder);
        if let Some(host) = host_data().filter(|folder| folder.is_dir()) {
            for name in OWN_SETTINGS {
                // The harness sees the file that a link names, and not the
                // link. A link that names nothing is left out. The file can
                // be in a folder that the agent can change, so it is
                // read-only at its own path too.
                if let Ok(found) = host.join(name).canonicalize() {
                    sandbox
                        .read_only(&found)
                        .read_only_at(&found, &self.folder.join(name));
                }
            }
            if !login {
                // With a token, the harness can renew no login of the host,
                // and nothing in the sandbox can read the login.
                sandbox.hidden(&host.join(LOGIN));
            } else if let Ok(file) = host.join(LOGIN).canonicalize()
                && file.is_file()
            {
                sandbox.writable_at(&file, &self.folder.join(LOGIN));
            }
        }
        let configuration = absolute(DATA_FOLDER)
            .or_else(|| absolute("HOME"))
            .map(|folder| folder.join(CONFIGURATION));
        if let Some(configuration) = configuration.filter(|file| file.is_file()) {
            let copy = self.folder.join(CONFIGURATION);
            fs::copy(&configuration, &copy)
                .and_then(|_| fs::set_permissions(&copy, fs::Permissions::from_mode(0o600)))
                .map_err(|error| {
                    format!(
                        "Lira can't copy `{}` for the harness: {error}",
                        configuration.display()
                    )
                })?;
        }
        Ok(())
    }
}

impl Drop for Data {
    fn drop(&mut self) {
        remove(&self.folder);
    }
}

/// Removes each folder in `above` that no process of Lira holds a lock on.
/// Lira holds the lock while it removes the folder, so that no other
/// process takes it.
fn remove_left(above: &Path) {
    for entry in fs::read_dir(above).into_iter().flatten().flatten() {
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let folder = entry.path();
        let opened = File::open(&folder).or_else(|error| {
            // The harness can take the right to read away from its folder.
            if error.kind() != ErrorKind::PermissionDenied {
                return Err(error);
            }
            fs::set_permissions(&folder, fs::Permissions::from_mode(0o700))?;
            File::open(&folder)
        });
        if let Ok(lock) = opened
            && lock.try_lock().is_ok()
        {
            remove(&folder);
        }
    }
}

/// Removes a data folder with all that is in it.
fn remove(folder: &Path) {
    if fs::remove_dir_all(folder).is_err() {
        // The harness can take the right to write away from a folder in it,
        // and then nothing in that folder can go.
        open(folder);
        let _ = fs::remove_dir_all(folder);
    }
}

/// Gives you the right to change `folder`, and each folder in it. It does
/// not follow links.
fn open(folder: &Path) {
    let _ = fs::set_permissions(folder, fs::Permissions::from_mode(0o700));
    for entry in fs::read_dir(folder).into_iter().flatten().flatten() {
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            open(&entry.path());
        }
    }
}

/// The data folder of the harness on the host, such as for its login and
/// its conversations. Claude Code keeps its data in `CLAUDE_CONFIG_DIR`, or
/// in `~/.claude` when that is not set.
fn host_data() -> Option<PathBuf> {
    absolute(DATA_FOLDER).or_else(|| absolute("HOME").map(|home| home.join(".claude")))
}

/// The value of the variable `name`, when it is an absolute path.
fn absolute(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .map(PathBuf::from)
        .filter(|folder| folder.is_absolute())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_data_folder_removes_the_left_ones_and_keeps_the_held_ones() {
        let above = std::env::temp_dir().join(format!("lira-data-{}", std::process::id()));
        let _ = fs::remove_dir_all(&above);
        let left = above.join("1-0");
        fs::create_dir_all(left.join("projects")).unwrap();
        fs::write(above.join("file"), "").unwrap();
        // A harness can take the right to read away from its data folder.
        let closed = above.join("1-1");
        fs::create_dir_all(&closed).unwrap();
        fs::set_permissions(&closed, fs::Permissions::from_mode(0o000)).unwrap();

        let held = Data::new_in(&above).unwrap();
        assert!(!left.exists());
        assert!(!closed.exists());
        assert!(above.join("file").exists());
        fs::set_permissions(&held.folder, fs::Permissions::from_mode(0o000)).unwrap();
        let other = Data::new_in(&above).unwrap();
        assert!(held.folder.exists());
        assert_ne!(held.folder, other.folder);

        let folder = held.folder.clone();
        drop(held);
        assert!(!folder.exists());
        assert!(other.folder.is_dir());
        drop(other);
        fs::remove_dir_all(&above).unwrap();
    }
}
