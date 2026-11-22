//! The sandbox of an agent. The harness runs in it, and it can change files
//! only in the folders that Lira names. It can read all files of the host.
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
#[derive(Default)]
pub struct Sandbox {
    writable: Vec<PathBuf>,
    read_only: Vec<PathBuf>,
}

impl Sandbox {
    /// A sandbox where the agent can change no file.
    pub fn new() -> Sandbox {
        Sandbox::default()
    }

    /// The agent can change what is in this folder or file.
    pub fn writable(&mut self, path: &Path) -> &mut Sandbox {
        self.writable.push(path.to_path_buf());
        self
    }

    /// The agent can't change this folder or file, also when it is in a
    /// writable folder. It must be there.
    pub fn read_only(&mut self, path: &Path) -> &mut Sandbox {
        self.read_only.push(path.to_path_buf());
        self
    }

    /// A command that runs `program` in the sandbox. Its folder is the
    /// folder that the command is given.
    pub fn command(&self, program: &OsStr) -> Command {
        let mut command = base();
        for path in &self.writable {
            command.arg("--bind").arg(path).arg(path);
        }
        // After the writable folders, so that these win.
        for path in &self.read_only {
            command.arg("--ro-bind").arg(path).arg(path);
        }
        command.arg("--").arg(program);
        command
    }
}

/// `bwrap` with what each sandbox has: all files of the host, read-only;
/// its own `/dev` and `/proc`; and an empty `/tmp` and `/run` of its own,
/// which go away with the sandbox. The sandbox ends when Lira stops `bwrap`.
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
