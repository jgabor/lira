//! The sandbox of an agent. The harness runs in it, and so does the check of
//! the change of the agent. What runs in it can change files only in the
//! folders that Lira names. It can read all files of the host, except the
//! logins for GitHub.
//!
//! This is the one place that knows how the sandbox works. Lira makes it
//! with the program `bwrap` (bubblewrap), so it runs on Linux only.

use crate::process;
use std::ffi::OsStr;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The program that makes the sandbox.
const BWRAP: &str = "bwrap";

/// The folders and files that an agent can change, and the ones in them
/// that it can't.
#[derive(Clone, Default)]
pub struct Sandbox {
    /// Each path of the host, the path where the agent sees it, and if the
    /// agent can change it. A later path inside an earlier one wins there.
    paths: Vec<(PathBuf, PathBuf, bool)>,
}

impl Sandbox {
    /// A sandbox where the agent can change no file.
    pub fn new() -> Sandbox {
        Sandbox::default()
    }

    /// The agent can change what is in this folder or file, also when it
    /// is in a read-only folder that came before.
    pub fn writable(&mut self, path: &Path) -> &mut Sandbox {
        self.writable_at(path, path)
    }

    /// The agent can't change this folder or file, also when it is in a
    /// writable folder that came before. It must be there.
    pub fn read_only(&mut self, path: &Path) -> &mut Sandbox {
        self.read_only_at(path, path)
    }

    /// The agent sees the folder or file `path` at `place` too, and can
    /// change it there, as with `writable`. What was at `place` is not
    /// there for the agent.
    pub fn writable_at(&mut self, path: &Path, place: &Path) -> &mut Sandbox {
        self.paths
            .push((path.to_path_buf(), place.to_path_buf(), true));
        self
    }

    /// The agent sees the folder or file `path` at `place` too, and can't
    /// change it there, as with `read_only`. What was at `place` is not
    /// there for the agent.
    pub fn read_only_at(&mut self, path: &Path, place: &Path) -> &mut Sandbox {
        self.paths
            .push((path.to_path_buf(), place.to_path_buf(), false));
        self
    }

    /// A command that runs `program` in the sandbox. Its folder is the
    /// folder that the command is given.
    pub fn command(&self, program: &OsStr) -> Command {
        let mut command = base();
        for (path, place, writable) in &self.paths {
            let bind = if *writable { "--bind" } else { "--ro-bind" };
            command.arg(bind).arg(path).arg(place);
        }
        hide_logins(&mut command);
        command.arg("--").arg(program);
        command
    }
}

/// The variables that can hold a token for GitHub, for `gh` and for git.
const TOKENS: [&str; 4] = [
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GH_ENTERPRISE_TOKEN",
    "GITHUB_ENTERPRISE_TOKEN",
];

/// The variable that names the socket of the ssh agent.
const SSH_AGENT: &str = "SSH_AUTH_SOCK";

/// Hides the logins for GitHub that the host keeps in the known places.
/// Lira keeps them, because Lira pushes the branch and opens the pull
/// request. A folder of logins is empty in the sandbox, the socket of the
/// ssh agent is not a socket there, and the variables with a token are not
/// set. This comes after the other paths, so that it wins over them.
///
/// A login in a folder that the sandbox has of its own is hidden already:
/// in `/dev`, `/proc`, `/tmp`, `/run`, or a folder of logins. `bwrap` would
/// make the folders of its path there, or hide a file of the sandbox, such
/// as `/dev/null`. So it is left out.
fn hide_logins(command: &mut Command) {
    let mut empty: Vec<PathBuf> = ["/dev", "/proc", "/tmp", "/run"]
        .iter()
        .map(PathBuf::from)
        .collect();
    let is_hidden =
        |path: &Path, empty: &[PathBuf]| empty.iter().any(|folder| path.starts_with(folder));
    for folder in login_folders() {
        if let Ok(folder) = folder.canonicalize()
            && folder.is_dir()
            && !is_hidden(&folder, &empty)
        {
            command.arg("--tmpfs").arg(&folder);
            empty.push(folder);
        }
    }
    let socket = std::env::var_os(SSH_AGENT).map(PathBuf::from);
    if let Some(socket) = socket.and_then(|socket| socket.canonicalize().ok())
        && !socket.is_dir()
        && !is_hidden(&socket, &empty)
    {
        command.arg("--ro-bind").arg("/dev/null").arg(socket);
    }
    for name in TOKENS.iter().chain([&SSH_AGENT]) {
        command.env_remove(name);
    }
}

/// The folders where `gh` and ssh keep logins for GitHub. `gh` keeps its
/// login in `GH_CONFIG_DIR`, or in `gh` in `XDG_CONFIG_HOME`, or in
/// `~/.config/gh`. Each of them is here, so that the agent finds no login
/// when it sets the variables in another way. `git push` gets its password
/// from `gh` too. ssh keeps its keys in `~/.ssh`.
fn login_folders() -> Vec<PathBuf> {
    let set = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());
    let mut folders = Vec::new();
    if let Some(folder) = set("GH_CONFIG_DIR") {
        folders.push(PathBuf::from(folder));
    }
    if let Some(folder) = set("XDG_CONFIG_HOME") {
        folders.push(Path::new(&folder).join("gh"));
    }
    if let Some(home) = set("HOME") {
        let home = Path::new(&home);
        folders.push(home.join(".config").join("gh"));
        folders.push(home.join(".ssh"));
    }
    folders
}

/// `bwrap` with what each sandbox has: all files of the host, read-only;
/// its own `/dev` and `/proc`; and an empty `/tmp` and `/run` of its own,
/// which go away with the sandbox. The sandbox ends when Lira stops `bwrap`.
///
/// What runs in the sandbox has no terminal: `--new-session` calls
/// `setsid`. Otherwise, on a host that allows `TIOCSTI`, it could type a
/// command into the terminal of Lira. `setsid` also takes it out of the
/// process group of `bwrap`, which is the one that Lira stops. So the
/// sandbox has its own processes too, with `--unshare-pid`: when `bwrap`
/// ends, the system stops each process in the sandbox.
///
/// A program can talk to a socket on a read-only folder, and the sockets of
/// the services of the host are in `/run`, such as the one of
/// `systemd-run --user`. A service could change a file for the agent, so the
/// agent sees none of them. The file of the name servers can be there too,
/// and the agent keeps it.
fn base() -> Command {
    let mut command = Command::new(BWRAP);
    command.args([
        "--ro-bind",
        "/",
        "/",
        "--dev",
        "/dev",
        "--proc",
        "/proc",
        "--tmpfs",
        "/tmp",
        "--tmpfs",
        "/run",
        "--die-with-parent",
        "--new-session",
        "--unshare-pid",
    ]);
    if let Ok(names) = Path::new("/etc/resolv.conf").canonicalize()
        && names.starts_with("/run")
    {
        command.arg("--ro-bind").arg(&names).arg(&names);
    }
    command
}

/// Makes one sandbox, to see that this host can. It is an error when the
/// host has no `bwrap`, or when `bwrap` makes no sandbox here. Lira runs no
/// agent with no sandbox.
pub fn require() -> Result<(), String> {
    let mut command = base();
    // `bwrap` itself is the program that runs in the sandbox, so this
    // needs no other program.
    command.args(["--", BWRAP, "--version"]);
    let ended = process::run(command, true).map_err(|error| match error.kind() {
        ErrorKind::NotFound => format!(
            "Lira runs each agent in a sandbox, and it makes the sandbox with `{BWRAP}` (bubblewrap). This host has no `{BWRAP}`, so Lira runs no agent. Install bubblewrap, or add `{BWRAP}` to PATH"
        ),
        _ => format!("Lira can't run `{BWRAP}`, so it makes no sandbox, and it runs no agent: {error}"),
    })?;
    if ended.status.success() {
        return Ok(());
    }
    Err(format!(
        "Lira runs each agent in a sandbox, and `{BWRAP}` made none on this host ({}), so Lira runs no agent. It wrote:\n{}",
        ended.status,
        String::from_utf8_lossy(&ended.output).trim_end()
    ))
}
