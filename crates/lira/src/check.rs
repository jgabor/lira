//! The check: the one command of a repository that says pass or fail for a
//! change. SPEC.md, "Check", and `lira check`.

use crate::lira_file::{self, LiraFile, Score};
use crate::scores::Scores;
use crate::{git, github};
use std::fs::{self, OpenOptions};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{self, Command};
use std::time::{SystemTime, UNIX_EPOCH};

/// What one run of the check command gave.
pub struct Outcome {
    pub passed: bool,
    /// The scores that the check wrote.
    pub scores: Scores,
}

/// Runs the check command of `file` in `folder`. `base` is the commit of the
/// main branch that the change is compared with.
pub fn run(file: &LiraFile, folder: &Path, base: &str) -> Result<Outcome, String> {
    let scores_file = empty_file()?;
    let status = Command::new("sh")
        .args(["-c", &file.check])
        .current_dir(folder)
        .env("LIRA_SCORES", &scores_file)
        .env("LIRA_BASE", base)
        .status();
    let written = fs::read_to_string(&scores_file);
    let _ = fs::remove_file(&scores_file);
    let status = status.map_err(|e| format!("couldn't run the check command with `sh`: {e}"))?;
    let written = written.map_err(|e| format!("couldn't read the scores of the check: {e}"))?;
    let scores = Scores::read(&written).map_err(|e| format!("the scores of the check, {e}"))?;
    let known = |name: &str| file.scores.iter().any(|score| score.name == name);
    if let Some(name) = scores.names().find(|name| !known(name)) {
        return Err(format!(
            "the check wrote the score `{name}`, and {} has no score with that name. Add it there as a `[[score]]`, or remove it from the check",
            lira_file::NAME
        ));
    }
    Ok(Outcome {
        passed: status.success(),
        scores,
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
    let text = match fs::read_to_string(folder.join(lira_file::NAME)) {
        Ok(text) => text,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            return Err(format!(
                "there is no `{}` in this folder. Run `lira check` in the root of a repository that has one",
                lira_file::NAME
            ));
        }
        Err(e) => return Err(format!("couldn't read `{}`: {e}", lira_file::NAME)),
    };
    let file = LiraFile::read(&text).map_err(|e| e.to_string())?;
    let main = git::main_branch(folder)?;
    let base = git::base(folder, &main)?;
    // A repository with no scores needs nothing from GitHub. Lira asks GitHub
    // before the check runs, so that a missing login stops it at once.
    let on_main = if file.scores.is_empty() {
        Scores::default()
    } else {
        github::main_scores(folder, main.strip_prefix("origin/").unwrap_or(&main))?
    };
    let outcome = run(&file, folder, &base)?;
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
