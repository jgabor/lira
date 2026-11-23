//! The sandbox of an agent. The harness runs in it, and so does the check of
//! the change of the agent. What runs in it can change files only in the
//! folders that Lira names. It can read all files of the host, except the
//! logins for GitHub.
//!
//! This is the one place that knows how the sandbox works. Lira makes it
//! with the program `bwrap` (bubblewrap), so it runs on Linux only.

use crate::process;
use std::ffi::{OsStr, OsString};
use std::io::ErrorKind;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
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
    /// Other files of logins, which the agent finds empty, as the logins
    /// for GitHub.
    hidden: Vec<PathBuf>,
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

    /// The agent finds this file of a login empty, and can't change it.
    pub fn hidden(&mut self, file: &Path) -> &mut Sandbox {
        self.hidden.push(file.to_path_buf());
        self
    }

    /// A command that runs `program` in the sandbox. Its folder is the
    /// folder that the command is given.
    pub fn command(&self, program: &OsStr) -> Command {
        let mut command = base();
        let user_folder = user_folder();
        if let Some(folder) = &user_folder {
            command.arg("--tmpfs").arg(folder);
        }
        for (path, place, writable) in &self.paths {
            let bind = if *writable { "--bind" } else { "--ro-bind" };
            command.arg(bind).arg(path).arg(place);
        }
        hide_logins(&mut command, &self.hidden, user_folder);
        command.arg("--").arg(program);
        command
    }
}

/// The folders that the sandbox has of its own. See `base`.
const OWN: [&str; 4] = ["/dev", "/proc", "/tmp", "/run"];

/// The variables that can hold a token for GitHub, for `gh` and for git.
const TOKENS: [&str; 4] = [
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GH_ENTERPRISE_TOKEN",
    "GITHUB_ENTERPRISE_TOKEN",
];

/// The variable that names the socket of the ssh agent.
const SSH_AGENT: &str = "SSH_AUTH_SOCK";

/// The sockets of gpg-agent.
const GPG_AGENT: [&str; 4] = [
    "S.gpg-agent",
    "S.gpg-agent.ssh",
    "S.gpg-agent.extra",
    "S.gpg-agent.browser",
];

/// The variable that names the session bus of D-Bus. The keyring of the
/// system answers there, and `gh` keeps its token in the keyring on some
/// hosts.
const SESSION_BUS: &str = "DBUS_SESSION_BUS_ADDRESS";

/// Hides the logins for GitHub that the host keeps in the known places.
/// Lira keeps them, because Lira pushes the branch and opens the pull
/// request. A folder of logins is empty in the sandbox. A file of logins is
/// empty there, and a socket that leads to a login is not a socket there.
/// The variables with a token or a socket are not set. This comes after the
/// other paths, so that it wins over them.
///
/// `/dev/null` of the host takes the place of each file and socket. It is
/// bound with `--dev-bind`, because a program can't open a device on a mount
/// of `--ro-bind`. So a program reads nothing from it, and what it writes
/// there goes nowhere, and not to the file of the host.
///
/// A login in a folder that the sandbox has of its own is hidden already:
/// in `/dev`, `/proc`, `/tmp`, `/run`, the folder of `XDG_RUNTIME_DIR`, or
/// a folder of logins. `bwrap` would make the folders of its path there, or
/// hide a file of the sandbox, such as `/dev/null`. So it is left out.
///
/// `others` are other files of logins, which are hidden as the files of
/// logins for GitHub. `user_folder` is the folder of `XDG_RUNTIME_DIR`,
/// when the sandbox has its own.
fn hide_logins(command: &mut Command, others: &[PathBuf], user_folder: Option<PathBuf>) {
    let mut hidden: Vec<PathBuf> = OWN.iter().map(PathBuf::from).chain(user_folder).collect();
    let is_hidden =
        |path: &Path, hidden: &[PathBuf]| hidden.iter().any(|folder| path.starts_with(folder));
    for folder in login_folders() {
        if let Ok(folder) = folder.canonicalize()
            && folder.is_dir()
            && !is_hidden(&folder, &hidden)
        {
            command.arg("--tmpfs").arg(&folder);
            hidden.push(folder);
        }
    }
    for file in login_files().iter().chain(others) {
        if let Ok(file) = file.canonicalize()
            && !file.is_dir()
            && !is_hidden(&file, &hidden)
        {
            command.arg("--dev-bind").arg("/dev/null").arg(&file);
            hidden.push(file);
        }
    }
    for name in TOKENS.iter().chain([&SSH_AGENT, &SESSION_BUS]) {
        command.env_remove(name);
    }
}

/// The value of a variable, when it is set and not empty.
fn set(name: &str) -> Option<OsString> {
    std::env::var_os(name).filter(|value| !value.is_empty())
}

/// The folder that `XDG_RUNTIME_DIR` names, when the sandbox needs to make
/// it empty. It has the sockets of the session bus and of the ssh agent of
/// the keyring. It is in `/run` on most hosts, and the sandbox has its own
/// `/run`, so then it is `None`. A relative path names no folder, as the
/// XDG specification says.
///
/// The sandbox makes it empty before it adds the paths that Lira names, as
/// it does `/tmp` and `/run`. So a path that Lira names in it is there.
fn user_folder() -> Option<PathBuf> {
    let folder = PathBuf::from(set("XDG_RUNTIME_DIR")?);
    let folder = folder
        .is_absolute()
        .then_some(folder)?
        .canonicalize()
        .ok()?;
    (folder.is_dir() && !OWN.iter().any(|path| folder.starts_with(path))).then_some(folder)
}

/// The home folders of the user: the one that `HOME` names, and the one
/// that the system names. They can differ, and ssh reads its keys from the
/// one that the system names.
fn homes() -> Vec<PathBuf> {
    let mut homes: Vec<PathBuf> = set("HOME").map(PathBuf::from).into_iter().collect();
    homes.extend(system_home().filter(|home| !homes.contains(home)));
    homes
}

/// The folders where `gh`, ssh and git keep logins for GitHub. `gh` keeps
/// its login in `GH_CONFIG_DIR`, or in `gh` in `XDG_CONFIG_HOME`, or in
/// `~/.config/gh`. Each of them is here, so that the agent finds no login
/// when it sets the variables in another way. `git push` gets its password
/// from `gh` too. ssh keeps its keys in `~/.ssh`. `git credential-cache`
/// keeps its socket in `~/.git-credential-cache`, or in `git/credential`
/// in `XDG_CACHE_HOME` or `~/.cache`. Git Credential Manager keeps its
/// plain store in `~/.gcm/store`.
fn login_folders() -> Vec<PathBuf> {
    let mut folders = Vec::new();
    if let Some(folder) = set("GH_CONFIG_DIR") {
        folders.push(PathBuf::from(folder));
    }
    if let Some(folder) = set("XDG_CONFIG_HOME") {
        folders.push(Path::new(&folder).join("gh"));
    }
    if let Some(folder) = set("XDG_CACHE_HOME") {
        folders.push(Path::new(&folder).join("git").join("credential"));
    }
    for home in homes() {
        folders.push(home.join(".config").join("gh"));
        folders.push(home.join(".ssh"));
        folders.push(home.join(".git-credential-cache"));
        folders.push(home.join(".cache").join("git").join("credential"));
        folders.push(home.join(".gcm").join("store"));
    }
    folders
}

/// The files and sockets that hold or lead to a login for GitHub. `git
/// credential-store` keeps its logins in `~/.git-credentials`, or in
/// `git/credentials` in `XDG_CONFIG_HOME` or `~/.config`. git also reads
/// `~/.netrc`. The ssh agent answers at the socket that `SSH_AUTH_SOCK`
/// names, and the keyring at the socket of the session bus. gpg-agent
/// answers at its sockets, and with its ssh support on, one of them leads
/// to the keys of ssh. It keeps them in `/run/user/<uid>/gnupg`, or, when
/// the host has no such folder, in `GNUPGHOME` or in `~/.gnupg`.
fn login_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Some(folder) = set("XDG_CONFIG_HOME") {
        files.push(Path::new(&folder).join("git").join("credentials"));
    }
    let mut gnupg: Vec<PathBuf> = set("GNUPGHOME").map(PathBuf::from).into_iter().collect();
    for home in homes() {
        files.push(home.join(".git-credentials"));
        files.push(home.join(".config").join("git").join("credentials"));
        files.push(home.join(".netrc"));
        gnupg.push(home.join(".gnupg"));
    }
    for folder in gnupg {
        files.extend(GPG_AGENT.iter().map(|name| folder.join(name)));
    }
    files.extend(set(SSH_AGENT).map(PathBuf::from));
    files.extend(session_buses());
    files
}

/// The sockets of the session bus of D-Bus: each `unix:path=` of
/// `DBUS_SESSION_BUS_ADDRESS`. A bus on an abstract socket or on the
/// network has no path, so it is not here. A program looks for `bus` in
/// `XDG_RUNTIME_DIR` when the variable is not set, and the sandbox hides
/// that folder.
fn session_buses() -> Vec<PathBuf> {
    set(SESSION_BUS)
        .map(|addresses| socket_paths(addresses.as_bytes()))
        .unwrap_or_default()
}

/// The paths of the sockets in a list of D-Bus addresses, such as
/// `unix:path=/run/user/1000/bus,guid=…`. The addresses are separated by
/// `;`, and a value can escape a byte as `%` and two hexadecimal digits.
fn socket_paths(addresses: &[u8]) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for address in addresses.split(|&byte| byte == b';') {
        let Some(keys) = address.strip_prefix(b"unix:") else {
            continue;
        };
        for pair in keys.split(|&byte| byte == b',') {
            if let Some(value) = pair.strip_prefix(b"path=")
                && let Some(path) = unescape(value)
            {
                paths.push(PathBuf::from(OsString::from_vec(path)));
            }
        }
    }
    paths
}

/// A value of a D-Bus address with each `%` and two hexadecimal digits as
/// the byte that they escape. It is `None` when a `%` escapes no byte.
fn unescape(value: &[u8]) -> Option<Vec<u8>> {
    let digit = |byte: u8| (byte as char).to_digit(16);
    let mut bytes = Vec::with_capacity(value.len());
    let mut rest = value.iter();
    while let Some(&byte) = rest.next() {
        if byte == b'%' {
            let high = digit(*rest.next()?)?;
            let low = digit(*rest.next()?)?;
            bytes.push((high * 16 + low) as u8);
        } else {
            bytes.push(byte);
        }
    }
    Some(bytes)
}

/// The home folder that the system names for the user, with
/// `getpwuid_r`, as ssh finds it.
fn system_home() -> Option<PathBuf> {
    use std::ffi::{CStr, c_char, c_int};
    use std::mem::MaybeUninit;
    use std::ptr;

    /// `struct passwd` of Linux.
    #[repr(C)]
    struct Passwd {
        name: *mut c_char,
        password: *mut c_char,
        uid: u32,
        gid: u32,
        gecos: *mut c_char,
        dir: *mut c_char,
        shell: *mut c_char,
    }
    unsafe extern "C" {
        fn getuid() -> u32;
        fn getpwuid_r(
            uid: u32,
            entry: *mut Passwd,
            buffer: *mut c_char,
            size: usize,
            result: *mut *mut Passwd,
        ) -> c_int;
    }
    /// The error of `getpwuid_r` when the buffer is too small.
    const ERANGE: c_int = 34;

    let mut entry = MaybeUninit::<Passwd>::uninit();
    let mut buffer: Vec<c_char> = vec![0; 4096];
    let mut result: *mut Passwd = ptr::null_mut();
    loop {
        // SAFETY: each pointer is valid for the call, and `size` is the
        // length of `buffer`.
        let error = unsafe {
            getpwuid_r(
                getuid(),
                entry.as_mut_ptr(),
                buffer.as_mut_ptr(),
                buffer.len(),
                &mut result,
            )
        };
        if error == ERANGE && buffer.len() < 1 << 20 {
            buffer.resize(buffer.len() * 2, 0);
            continue;
        }
        if error != 0 || result.is_null() {
            return None;
        }
        break;
    }
    // SAFETY: `getpwuid_r` found the entry, so `result` points at `entry`,
    // and its strings are in `buffer`, which lives until the end of this
    // function.
    let dir = unsafe { (*result).dir };
    if dir.is_null() {
        return None;
    }
    // SAFETY: `dir` is a string that ends with a zero byte, in `buffer`.
    let dir = unsafe { CStr::from_ptr(dir) };
    let dir = OsStr::from_bytes(dir.to_bytes());
    (!dir.is_empty()).then(|| PathBuf::from(dir))
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
/// Its own processes also keep the host out of its `/proc`. With a setuid
/// `bwrap`, the sandbox has no user namespace of its own, and
/// `/proc/<number>/root` of a process outside the sandbox would show the
/// files of the host, with the logins, and with files that the agent can
/// change.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_path_of_each_unix_socket_in_a_list_of_addresses() {
        let addresses = b"unix:abstract=/tmp/a,guid=1;tcp:host=h,port=1;\
            unix:path=/home/u/bus,guid=1;unix:guid=2,path=/s%2c%ffx";
        assert_eq!(
            socket_paths(addresses),
            [
                PathBuf::from("/home/u/bus"),
                PathBuf::from(OsString::from_vec(b"/s,\xffx".to_vec())),
            ]
        );
    }

    #[test]
    fn a_path_with_a_bad_escape_is_no_path() {
        assert_eq!(socket_paths(b"unix:path=/s%2"), Vec::<PathBuf>::new());
        assert_eq!(socket_paths(b"unix:path=/s%zz"), Vec::<PathBuf>::new());
    }
}
