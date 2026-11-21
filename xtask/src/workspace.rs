//! Where the workspace is, and which packages it has.

use crate::Problem;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn no_cargo(error: io::Error) -> Problem {
    Problem(format!(
        "couldn't run `cargo`: {error}. Install Rust, or add cargo to PATH"
    ))
}

pub fn root() -> Result<PathBuf, Problem> {
    let output = Command::new("cargo")
        .args(["locate-project", "--workspace", "--message-format", "plain"])
        .output()
        .map_err(no_cargo)?;
    if !output.status.success() {
        return Err(Problem(format!(
            "not inside a Cargo workspace: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let manifest = String::from_utf8_lossy(&output.stdout);
    let manifest = Path::new(manifest.trim());
    let root = manifest
        .parent()
        .ok_or_else(|| Problem(format!("`{}` has no parent", manifest.display())))?;
    Ok(root.to_path_buf())
}

#[derive(Debug, PartialEq, Eq)]
pub struct Package {
    pub name: String,
    pub directory: PathBuf,
}

/// The packages of the workspace, in the order Cargo reports them.
pub fn packages(root: &Path) -> Result<Vec<Package>, Problem> {
    // With depth 0, `cargo tree` prints one line for each package of the
    // workspace, and nothing for what they depend on.
    let output = Command::new("cargo")
        .args(["tree", "--workspace", "--locked", "--depth", "0"])
        .args(["--prefix", "none", "--format", "{p}"])
        .current_dir(root)
        .output()
        .map_err(no_cargo)?;
    if !output.status.success() {
        return Err(Problem(format!(
            "`cargo tree` failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let mut packages: Vec<Package> = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if line.trim().is_empty() {
            continue;
        }
        let package = parse_package(line)
            .ok_or_else(|| Problem(format!("unexpected `cargo tree` output: {line}")))?;
        if !packages.contains(&package) {
            packages.push(package);
        }
    }
    Ok(packages)
}

/// Reads a line such as `lira v0.0.0 (/home/you/lira/crates/lira)` into the
/// name and the folder of the package.
fn parse_package(line: &str) -> Option<Package> {
    let (name, rest) = line.trim().split_once(' ')?;
    let directory = rest.strip_suffix(')')?.rsplit_once('(')?.1;
    Some(Package {
        name: name.to_string(),
        directory: PathBuf::from(directory),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(name: &str, directory: &str) -> Option<Package> {
        Some(Package {
            name: name.to_string(),
            directory: PathBuf::from(directory),
        })
    }

    #[test]
    fn reads_a_package_line() {
        assert_eq!(
            parse_package("lira v0.0.0 (/home/you/lira/crates/lira)"),
            package("lira", "/home/you/lira/crates/lira")
        );
    }

    #[test]
    fn reads_the_folder_after_another_note() {
        assert_eq!(
            parse_package("derive v0.0.0 (proc-macro) (/work/crates/derive)"),
            package("derive", "/work/crates/derive")
        );
    }

    #[test]
    fn a_line_without_a_folder_is_not_a_package() {
        assert_eq!(parse_package("lira v0.0.0"), None);
    }

    #[test]
    fn finds_the_packages_of_this_workspace() {
        let root = root().unwrap();
        let names: Vec<String> = packages(&root)
            .unwrap()
            .into_iter()
            .map(|package| package.name)
            .collect();
        assert!(names.contains(&"lira".to_string()), "{names:?}");
        assert!(names.contains(&"xtask".to_string()), "{names:?}");
    }
}
