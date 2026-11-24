//! The data folder: where Lira keeps what it makes on the host.

use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The data folder: `$XDG_DATA_HOME/lira`, or `~/.local/share/lira` when
/// `XDG_DATA_HOME` is not set.
pub fn folder() -> Result<PathBuf, String> {
    from(env::var_os("XDG_DATA_HOME"), env::var_os("HOME"))
}

/// The folder of the locks that Lira takes on the work of agents, in the
/// data folder `data`.
pub fn locks(data: &Path) -> PathBuf {
    data.join("locks")
}

/// The folder of the data folders of the harnesses of agents, and of their
/// lock files, in the data folder `data`.
pub fn harnesses(data: &Path) -> PathBuf {
    data.join("harness")
}

/// The data folder for these values of `XDG_DATA_HOME` and `HOME`.
///
/// A value that is empty, or that is no absolute path, counts as not set.
/// The XDG Base Directory Specification says so for `XDG_DATA_HOME`, and a
/// path that is not absolute would move with the folder that Lira starts
/// in.
fn from(xdg: Option<OsString>, home: Option<OsString>) -> Result<PathBuf, String> {
    let absolute = |value: Option<OsString>| {
        value
            .map(PathBuf::from)
            .filter(|folder| folder.is_absolute())
    };
    if let Some(xdg) = absolute(xdg) {
        return Ok(xdg.join("lira"));
    }
    match absolute(home) {
        Some(home) => Ok(home.join(".local").join("share").join("lira")),
        None => Err(
            "Lira can't find its data folder. Set `XDG_DATA_HOME`, or `HOME`, to the absolute path of a folder"
                .to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(text: &str) -> Option<OsString> {
        Some(OsString::from(text))
    }

    #[test]
    fn the_data_folder_is_in_xdg_data_home_or_in_the_home_folder() {
        let folder = |xdg, home| from(xdg, home).map_err(|_| ());
        assert_eq!(
            folder(value("/x"), value("/h")),
            Ok(PathBuf::from("/x/lira"))
        );
        assert_eq!(
            folder(None, value("/h")),
            Ok(PathBuf::from("/h/.local/share/lira"))
        );
        assert_eq!(
            folder(value(""), value("/h")),
            Ok(PathBuf::from("/h/.local/share/lira"))
        );
        assert_eq!(
            folder(value("x"), value("/h")),
            Ok(PathBuf::from("/h/.local/share/lira"))
        );
        assert_eq!(folder(value("/x"), None), Ok(PathBuf::from("/x/lira")));
        assert_eq!(folder(None, None), Err(()));
        assert_eq!(folder(None, value("h")), Err(()));
        assert_eq!(folder(value("x"), value("")), Err(()));
    }
}
