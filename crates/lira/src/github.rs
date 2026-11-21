//! What Lira asks GitHub, through the GitHub command line, `gh`. Lira has no
//! token of its own: `gh` uses its login.

use crate::scores::Scores;
use std::io::ErrorKind;
use std::path::Path;
use std::process::{Command, Stdio};

/// What `gh --jq` prints after each comment, so that Lira can tell one
/// comment from the next.
const END_OF_COMMENT: char = '\u{1e}';

/// Stops with an error that names `gh auth login` when `gh` has no login.
fn require_login(folder: &Path) -> Result<(), String> {
    let status = Command::new("gh")
        .args(["auth", "status"])
        .current_dir(folder)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match status {
        Ok(status) if status.success() => Ok(()),
        Ok(_) => {
            Err("the GitHub command line, `gh`, has no login. Run `gh auth login`".to_string())
        }
        Err(e) if e.kind() == ErrorKind::NotFound => Err(
            "Lira needs the GitHub command line, `gh`. Install it, then run `gh auth login`"
                .to_string(),
        ),
        Err(e) => Err(format!("couldn't run `gh`: {e}")),
    }
}

/// Asks the GitHub API of the repository in `folder`, and returns what the
/// `jq` expression prints.
fn api(folder: &Path, path: &str, jq: &str) -> Result<String, String> {
    let output = Command::new("gh")
        .args(["api", path, "--jq", jq])
        .current_dir(folder)
        .output()
        .map_err(|e| format!("couldn't run `gh`: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "`gh api {path}` failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The scores of the main branch: those in the comment on the last merged
/// pull request. `branch` is the name of the main branch on GitHub. With no
/// merged pull request, or no such comment on it, no score has a value.
pub fn main_scores(folder: &Path, branch: &str) -> Result<Scores, String> {
    require_login(folder)?;
    let pulls = format!(
        "repos/{{owner}}/{{repo}}/pulls?state=closed&base={branch}&sort=updated&direction=desc&per_page=100"
    );
    let newest =
        "[.[] | select(.merged_at != null)] | sort_by(.merged_at) | last | .number // empty";
    let number = api(folder, &pulls, newest)?;
    let number = number.trim();
    if number.is_empty() {
        return Ok(Scores::default());
    }
    let comments = format!("repos/{{owner}}/{{repo}}/issues/{number}/comments?per_page=100");
    let bodies = api(
        folder,
        &comments,
        &format!(".[] | .body + \"{END_OF_COMMENT}\""),
    )?;
    Ok(scores_comment(&bodies))
}

/// The newest comment that is only score lines.
fn scores_comment(bodies: &str) -> Scores {
    let comments = bodies.split(END_OF_COMMENT).rev();
    let mut scores = comments.filter_map(|body| Scores::read(body).ok());
    scores.find(|scores| !scores.is_empty()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scores_are_in_the_newest_comment_that_is_only_score_lines() {
        let bodies = "spec = 1\n\u{1e}\nGood work. spec = 5 now.\n\u{1e}\nspec = 14\r\nsize = 2\n\u{1e}\nThanks!\n\u{1e}\n";
        let scores = scores_comment(bodies);
        assert_eq!(scores.names().collect::<Vec<_>>(), ["spec", "size"]);
        assert_eq!(scores.get("spec").unwrap().to_string(), "14");
    }

    #[test]
    fn with_no_such_comment_no_score_has_a_value() {
        assert!(scores_comment("").is_empty());
        assert!(scores_comment("Thanks!\n\u{1e}\n").is_empty());
    }
}
