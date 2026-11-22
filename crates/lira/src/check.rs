//! The check: the one command of a repository that says pass or fail for a
//! change. SPEC.md, "Check", and `lira check`.

use crate::lira_file::{self, LiraFile, Score};
use crate::sandbox::Sandbox;
use crate::scores::Scores;
use crate::{git, github};
use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{self, Command};
use std::time::{SystemTime, UNIX_EPOCH};

/// What one run of the check command gave.
pub struct Outcome {
    pub passed: bool,
    /// The scores that the check wrote.
    pub scores: Scores,
    /// How the check command ended, such as `exit status: 1`.
    pub status: String,
    /// What the check command wrote, to its output and to its error output,
    /// when Lira kept it.
    pub wrote: String,
}

/// What Lira does with what the check command writes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Output {
    /// The person who runs Lira reads it, as the check writes it.
    Shown,
    /// Lira keeps it, to give it to an agent.
    Kept,
}

/// Runs the check command of `file` in `folder`. `base` is the commit of the
/// main branch that the change is compared with. With a `sandbox`, the
/// check runs in it, and it can also write the file of the scores.
pub fn run(
    file: &LiraFile,
    folder: &Path,
    base: &str,
    output: Output,
    sandbox: Option<&Sandbox>,
) -> Result<Outcome, String> {
    let scores_file = empty_file()?;
    let mut command = match sandbox {
        Some(sandbox) => {
            let mut sandbox = sandbox.clone();
            sandbox.writable(&scores_file);
            sandbox.command(OsStr::new("sh"))
        }
        None => Command::new("sh"),
    };
    command
        .args(["-c", &file.check])
        .current_dir(folder)
        .env("LIRA_SCORES", &scores_file)
        .env("LIRA_BASE", base);
    let ended = match output {
        Output::Shown => command.status().map(|status| (status, Vec::new())),
        // The check has no input here, and Lira does not wait for a process
        // that the check left behind.
        Output::Kept => {
            crate::process::run(command, true).map(|ended| (ended.status, ended.output))
        }
    };
    let written = fs::read_to_string(&scores_file);
    let _ = fs::remove_file(&scores_file);
    let (status, wrote) =
        ended.map_err(|e| format!("couldn't run the check command with `sh`: {e}"))?;
    let read = || {
        let written = written.map_err(|e| format!("couldn't read the scores of the check: {e}"))?;
        let scores = Scores::read(&written).map_err(|e| format!("the scores of the check, {e}"))?;
        let known = |name: &str| file.scores.iter().any(|score| score.name == name);
        let unknown = scores.names().find(|name| !known(name)).map(str::to_string);
        match unknown {
            Some(name) => Err(format!(
                "the check wrote the score `{name}`, and {} has no score with that name. Add it there as a `[[score]]`, or remove it from the check",
                lira_file::NAME
            )),
            None => Ok(scores),
        }
    };
    let mut wrote = String::from_utf8_lossy(&wrote).into_owned();
    let (scores, passed) = match (read(), output) {
        (Ok(scores), _) => (scores, status.success()),
        (Err(error), Output::Shown) => return Err(error),
        // An agent reads what the check wrote, and it can repair the
        // scores too. So scores that Lira can't use are a failed check, and
        // what is wrong with them comes after the output.
        (Err(error), Output::Kept) => {
            wrote.push_str(&format!(
                "\nLira can't use the scores that the check wrote: {error}\n"
            ));
            (Scores::default(), false)
        }
    };
    Ok(Outcome {
        passed,
        scores,
        status: status.to_string(),
        wrote,
    })
}

/// Makes a new empty file for the scores, and returns its path.
fn empty_file() -> Result<PathBuf, String> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let path = std::env::temp_dir().join(format!("lira-scores-{}-{nanos}", process::id()));
    let made = OpenOptions::new().write(true).create_new(true).open(&path);
    made.map_err(|e| {
        format!(
            "couldn't make the file for the scores, {}: {e}",
            path.display()
        )
    })?;
    Ok(path)
}

/// `lira check`: runs the check command of the Lira file in `folder`, and
/// prints each score. Returns true when the check passes.
pub fn command(folder: &Path) -> Result<bool, String> {
    let main = git::main_branch(folder)?;
    // Lira reads the Lira file from the main branch, so that a change can't
    // replace its own check.
    let file = git::lira_file(folder, &main)?;
    let base = git::base(folder, &main.full)?;
    // A repository with no scores needs nothing from GitHub. Lira asks GitHub
    // before the check runs, so that a missing login stops it at once.
    let on_main = if file.scores.is_empty() {
        Scores::default()
    } else {
        github::main_scores(folder, &main.name)?
    };
    let outcome = run(&file, folder, &base, Output::Shown, None)?;
    for line in report(&file.scores, &outcome.scores, &on_main) {
        println!("{line}");
    }
    Ok(outcome.passed)
}

/// One line for each score of the Lira file, with its value and its value on
/// the main branch.
fn report(scores: &[Score], values: &Scores, on_main: &Scores) -> Vec<String> {
    let line = |score: &Score| {
        let (value, main) = (values.get(&score.name), on_main.get(&score.name));
        let name = &score.name;
        let now = value.map_or(format!("{name} has no value"), |value| {
            format!("{name} = {value}")
        });
        let before = main.map_or("no value".to_string(), ToString::to_string);
        // A score with no value is not compared.
        let worse = matches!((value, main), (Some(value), Some(main)) if value.worse_than(main, score.better));
        format!(
            "{now}, and {before} on the main branch{}",
            if worse { ": worse" } else { "" }
        )
    };
    scores.iter().map(line).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lira_file::Better;

    fn score(name: &str, better: Better) -> Score {
        Score {
            name: name.to_string(),
            better,
        }
    }

    #[test]
    fn a_line_says_the_value_and_the_value_on_the_main_branch() {
        let scores = [
            score("spec", Better::Higher),
            score("size", Better::Lower),
            score("new", Better::Higher),
            score("gone", Better::Lower),
            score("never", Better::Lower),
        ];
        let values = Scores::read("spec = 13\nsize = 2\nnew = 1\n").unwrap();
        let on_main = Scores::read("spec = 14\nsize = 2\ngone = 9\n").unwrap();
        assert_eq!(
            report(&scores, &values, &on_main),
            [
                "spec = 13, and 14 on the main branch: worse",
                "size = 2, and 2 on the main branch",
                "new = 1, and no value on the main branch",
                "gone has no value, and 9 on the main branch",
                "never has no value, and no value on the main branch",
            ]
        );
    }
}
