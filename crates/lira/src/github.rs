//! What Lira asks GitHub, through the GitHub command line, `gh`. Lira has no
//! token of its own: `gh` uses its login.

use crate::scores::Scores;
use std::io::ErrorKind;
use std::path::Path;
use std::process::{Command, Stdio};

/// What `gh --jq` prints after each comment, so that Lira can tell one
/// comment from the next.
const END_OF_COMMENT: char = '\u{1e}';

/// How many of the newest commits of the main branch Lira asks about, to
/// find the last merged pull request.
const NEWEST_COMMITS: usize = 20;

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
/// `jq` expression prints. `more` is the other arguments of `gh api`.
fn api(folder: &Path, path: &str, more: &[&str], jq: &str) -> Result<String, String> {
    let path = format!("repos/{{owner}}/{{repo}}/{path}");
    let output = Command::new("gh")
        .args(["api", &path])
        .args(more)
        .args(["--jq", jq])
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
/// such pull request, or no such comment on it, no score has a value.
pub fn main_scores(folder: &Path, branch: &str) -> Result<Scores, String> {
    require_login(folder)?;
    let Some(number) = last_merged(folder, branch)? else {
        return Ok(Scores::default());
    };
    let comments = format!("issues/{number}/comments");
    let each = format!(".[] | .body + \"{END_OF_COMMENT}\"");
    let bodies = api(folder, &comments, &["--paginate"], &each)?;
    Ok(scores_comment(&bodies))
}

/// The number of the last pull request that was merged into `branch`: the
/// one that the newest commit of the branch came from. A commit that was
/// pushed with no pull request has none, so Lira looks at the commit before
/// it, for the newest commits.
fn last_merged(folder: &Path, branch: &str) -> Result<Option<String>, String> {
    let newest = [
        "--method",
        "GET",
        "-f",
        &format!("sha={branch}"),
        "-f",
        &format!("per_page={NEWEST_COMMITS}"),
    ];
    let commits = api(folder, "commits", &newest, ".[].sha")?;
    for commit in commits.lines() {
        let each = r#".[] | "\(.number) \(.merged_at // "open") \(.base.ref)""#;
        let pulls = api(folder, &format!("commits/{commit}/pulls"), &[], each)?;
        if let Some(number) = merged_into(&pulls, branch) {
            return Ok(Some(number));
        }
    }
    Ok(None)
}

/// Picks from lines of `<number> <time of the merge, or open> <branch>` the
/// pull request that was merged into `branch` last.
fn merged_into(pulls: &str, branch: &str) -> Option<String> {
    let merged = pulls.lines().filter_map(|line| {
        let mut parts = line.splitn(3, ' ');
        let (number, time, into) = (parts.next()?, parts.next()?, parts.next()?);
        (time != "open" && into == branch).then_some((time, number))
    });
    merged.max().map(|(_, number)| number.to_string())
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

    #[test]
    fn the_last_merged_pull_request_is_merged_and_into_the_main_branch() {
        let pulls = "5 open main\n8 2026-11-21T10:00:00Z release/1\n6 2026-11-18T10:00:00Z main\n7 2026-11-20T10:00:00Z main\n";
        assert_eq!(merged_into(pulls, "main"), Some("7".to_string()));
        assert_eq!(merged_into(pulls, "release/1"), Some("8".to_string()));
        assert_eq!(merged_into("5 open main\n", "main"), None);
        assert_eq!(merged_into("", "main"), None);
    }
}
