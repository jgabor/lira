//! An agent: a harness that runs, and one conversation with it. The agent
//! that does the work and each Reviewer start in the same way.

use crate::data;
use crate::harness::{Harness, Stop};
use crate::lira_file::LiraFile;
use crate::sandbox::Sandbox;
use std::collections::BTreeSet;
use std::fs::{self, DirBuilder, File, TryLockError};
use std::io::ErrorKind;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
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
/// data folder of Lira, and Lira removes it when the agent ends. Its name is
/// `<host>-<process>-<number>`, with the name of the host and the number of
/// the process of Lira.
///
/// The process of Lira that made the folder holds a lock on the file
/// `<folder>.lock` beside it, until the folder is gone. A process that ends
/// with no time to remove the folder, as after `SIGKILL`, loses the lock all
/// the same. So a folder with no lock is one that Lira left, and the next
/// agent on the same host removes it. The lock is on a file that Lira opens
/// to write, and not on the folder, because NFS takes only such a lock. The
/// harness can't remove a lock file, because the folder `harness` is
/// read-only in its sandbox. It can take the lock of a folder that Lira
/// left, which keeps that folder only while the harness runs.
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
    /// The open lock file, with the lock on it. `drop` removes the folder
    /// and the lock file before the lock goes away.
    lock: Lock,
}

impl Data {
    /// Makes a new data folder, which only you can read.
    fn new() -> Result<Data, String> {
        Data::new_in(&data::folder()?.join("harness"), &host()?)
    }

    /// Makes a new data folder in `above`, for the host with the name
    /// `host`. First it removes each folder there that Lira left on this
    /// host.
    fn new_in(above: &Path, host: &str) -> Result<Data, String> {
        let failed = |path: &Path, error| {
            format!("Lira can't make the folder `{}`: {error}", path.display())
        };
        fs::create_dir_all(above).map_err(|error| failed(above, error))?;
        remove_left(above, host);
        loop {
            let name = format!(
                "{host}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            );
            let folder = above.join(&name);
            // The lock comes first, so that no other process removes the
            // folder before it is complete. Another process holds the lock
            // only while it removes what an earlier process with the same
            // number left.
            let lock = match Lock::take(&folder) {
                Ok(Taken::Held(lock)) => lock,
                Ok(Taken::Busy) => continue,
                // With no lock, the next agent could remove the folder
                // while this agent works in it. So no agent runs.
                Err(error) => {
                    return Err(format!(
                        "Lira can't lock the file `{}`, which keeps the data folder of the harness while the agent works: {error}",
                        lock_file(&folder).display()
                    ));
                }
            };
            // The folder must be new: Lira removes it with all that is in it.
            match DirBuilder::new().mode(0o700).create(&folder) {
                Ok(()) => return Ok(Data { folder, lock }),
                // A folder that an earlier process with the same number
                // left. The next agent removes it.
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                    lock.keep();
                    continue;
                }
                Err(error) => return Err(failed(&folder, error)),
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
        self.lock.remove();
    }
}

/// The lock file of a data folder: `<folder>.lock`.
fn lock_file(folder: &Path) -> PathBuf {
    let mut path = folder.as_os_str().to_os_string();
    path.push(".lock");
    PathBuf::from(path)
}

/// The open lock file of a data folder. Lira removes the lock file when it
/// lets go of it.
struct Lock {
    path: PathBuf,
    file: Option<File>,
}

/// What Lira found when it tried to take a lock.
enum Taken {
    /// Lira has the lock.
    Held(Lock),
    /// Another process holds the lock.
    Busy,
}

impl Lock {
    /// Takes the lock of `folder`. It is an error when the file system
    /// takes no lock. Then Lira removes only a lock file that it made.
    fn take(folder: &Path) -> std::io::Result<Taken> {
        let path = lock_file(folder);
        let open = |new: bool| {
            fs::OpenOptions::new()
                .write(true)
                .create(true)
                .create_new(new)
                .truncate(false)
                .mode(0o600)
                .open(&path)
        };
        // If this call made the lock file, so that it can remove it again.
        let (file, made) = match open(true) {
            Ok(file) => (file, true),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => (open(false)?, false),
            Err(error) => return Err(error),
        };
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Ok(Taken::Busy),
            Err(TryLockError::Error(error)) => {
                // A file system that takes no lock gets no new lock files.
                if made {
                    let _ = fs::remove_file(&path);
                }
                return Err(error);
            }
        }
        // Another process can remove the lock file after Lira opened it,
        // and before Lira took the lock. Then the lock is on a file that is
        // gone, and it holds nothing.
        let same = |found: &fs::Metadata, held: &fs::Metadata| {
            found.dev() == held.dev() && found.ino() == held.ino()
        };
        match (fs::symlink_metadata(&path), file.metadata()) {
            (Ok(found), Ok(held)) if same(&found, &held) => Ok(Taken::Held(Lock {
                path,
                file: Some(file),
            })),
            _ => Ok(Taken::Busy),
        }
    }

    /// Removes the lock file, while the lock is still on it.
    fn remove(&mut self) {
        if let Some(file) = self.file.take() {
            let _ = fs::remove_file(&self.path);
            drop(file);
        }
    }

    /// Lets go of the lock, and keeps the lock file.
    fn keep(mut self) {
        self.file = None;
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        self.remove();
    }
}

/// Removes each data folder of the host `host` in `above` that no process
/// of Lira holds a lock on, and its lock file. Lira holds the lock while it
/// removes them, so that no other process takes them.
fn remove_left(above: &Path, host: &str) {
    let mut folders = BTreeSet::new();
    for entry in fs::read_dir(above).into_iter().flatten().flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let folder = match name.strip_suffix(".lock") {
            Some(folder) => folder,
            None if entry.file_type().is_ok_and(|kind| kind.is_dir()) => name,
            None => continue,
        };
        if of_host(folder, host) {
            folders.insert(folder.to_string());
        }
    }
    for name in folders {
        let folder = above.join(name);
        if let Ok(Taken::Held(mut lock)) = Lock::take(&folder) {
            // A lock file can be there with no folder, and Lira removes
            // only a folder, and not what a link names.
            if fs::symlink_metadata(&folder).is_ok_and(|found| found.is_dir()) {
                remove(&folder);
            }
            lock.remove();
        }
    }
}

/// If `name` is the name of a data folder that Lira made on the host with
/// the name `host`: `<host>-<number>-<number>`. Two hosts can share the
/// data folder of Lira, as on NFS, and a host can't always see the locks
/// of another host there. So Lira removes only the folders of its own host.
fn of_host(name: &str, host: &str) -> bool {
    let number = |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    let mut parts = name.rsplitn(3, '-');
    matches!(
        (parts.next(), parts.next(), parts.next()),
        (Some(next), Some(process), Some(named)) if number(next) && number(process) && named == host
    )
}

/// The name of the host, as Linux gives it.
fn host() -> Result<String, String> {
    let file = "/proc/sys/kernel/hostname";
    let name = fs::read_to_string(file)
        .map_err(|error| format!("Lira can't read the name of the host in `{file}`: {error}"))?;
    let name = name.trim();
    if name.is_empty() {
        return Err(format!(
            "Lira can't read the name of the host: `{file}` is empty"
        ));
    }
    Ok(name.to_string())
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
        let left = above.join("h-1-0");
        fs::create_dir_all(left.join("projects")).unwrap();
        fs::write(above.join("file"), "").unwrap();
        // A harness can take the right to read away from its data folder.
        let closed = above.join("h-1-1");
        fs::create_dir_all(&closed).unwrap();
        fs::set_permissions(&closed, fs::Permissions::from_mode(0o000)).unwrap();
        // The lock of a folder that Lira left, and a lock with no folder.
        fs::write(above.join("h-1-1.lock"), "").unwrap();
        fs::write(above.join("h-1-2.lock"), "").unwrap();
        // A lock file beside a file that is no data folder.
        fs::write(above.join("h-1-3"), "").unwrap();
        fs::write(above.join("h-1-3.lock"), "").unwrap();
        fs::set_permissions(above.join("h-1-3"), fs::Permissions::from_mode(0o644)).unwrap();
        // The folders of other hosts, and of a Lira that put no host in
        // the name. Lira can't see their locks.
        let others = ["g-1-0", "x-h-1-0", "1-0"].map(|name| above.join(name));
        for folder in &others {
            fs::create_dir_all(folder).unwrap();
        }
        fs::write(above.join("g-1-0.lock"), "").unwrap();

        let held = Data::new_in(&above, "h").unwrap();
        assert!(!left.exists());
        assert!(!closed.exists());
        assert!(!above.join("h-1-1.lock").exists());
        assert!(!above.join("h-1-2.lock").exists());
        let mode = fs::metadata(above.join("h-1-3"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o644);
        assert!(!above.join("h-1-3.lock").exists());
        assert!(above.join("file").exists());
        for folder in &others {
            assert!(folder.is_dir(), "{}", folder.display());
        }
        assert!(above.join("g-1-0.lock").exists());
        let name = held.folder.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with("h-"), "{name}");
        let lock = above.join(format!("{name}.lock"));
        assert!(lock.is_file());
        fs::set_permissions(&held.folder, fs::Permissions::from_mode(0o000)).unwrap();
        let other = Data::new_in(&above, "h").unwrap();
        assert!(held.folder.exists());
        // The folder that a harness holds keeps the mode that the harness gave it.
        let mode = fs::symlink_metadata(&held.folder)
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o000);
        assert_ne!(held.folder, other.folder);

        let folder = held.folder.clone();
        drop(held);
        assert!(!folder.exists());
        assert!(!lock.exists());
        assert!(other.folder.is_dir());
        drop(other);
        fs::remove_dir_all(&above).unwrap();
    }
}
