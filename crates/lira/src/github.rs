//! What Lira asks GitHub, through the GitHub command line, `gh`. Lira has no
//! token of its own: `gh` uses its login.

use crate::scores::Scores;
use std::ffi::OsStr;
use std::io::ErrorKind;
use std::path::Path;
use std::process::{Command, Stdio};

/// What `gh --jq` prints after each comment, so that Lira can tell one
/// comment from the next.
const END_OF_COMMENT: char = '\u{1e}';

/// Asks for one page of the commits of a branch, newest first, with the
/// pull requests that each commit came from, and for the count of the pull
/// requests that were merged into the branch.
const HISTORY: &str = "\
query($owner: String!, $name: String!, $branch: String!, $ref: String!, $after: String) {
  repository(owner: $owner, name: $name) {
    pullRequests(states: MERGED, baseRefName: $branch) { totalCount }
    ref(qualifiedName: $ref) { target { ... on Commit {
      history(first: 100, after: $after) {
        pageInfo { hasNextPage endCursor }
        nodes { associatedPullRequests(first: 20) { nodes { number mergedAt baseRefName } } }
      }
    } } }
  }
}";

/// Prints the answer to `HISTORY` as lines: `count <merged pull requests>`,
/// `next <where the next page starts, or nothing>`, and for each pull request
/// `pull <place of its commit> <number> <time of the merge, or open> <branch>`.
const HISTORY_LINES: &str = r#".data.repository as $r
| ($r.ref.target.history // {}) as $h
| "count \($r.pullRequests.totalCount)",
  "next \(if $h.pageInfo.hasNextPage then $h.pageInfo.endCursor else "" end)",
  (($h.nodes // []) | to_entries[] | .key as $place | .value.associatedPullRequests.nodes[]
    | "pull \($place) \(.number) \(.mergedAt // "open") \(.baseRefName)")"#;

/// Stops with an error that names `gh auth login` when `gh` has no login.
pub fn require_login(folder: &Path) -> Result<(), String> {
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

/// Runs `gh api` in `folder` with these arguments, and returns what it
/// prints.
fn api(folder: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("gh")
        .arg("api")
        .args(args)
        .current_dir(folder)
        .output()
        .map_err(|e| format!("couldn't run `gh`: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "`gh api {}` failed: {}",
            args[0],
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Runs `gh pr` in `folder` with these arguments, and returns what it
/// prints.
fn pull_request(folder: &Path, args: &[&OsStr]) -> Result<String, String> {
    let output = Command::new("gh")
        .arg("pr")
        .args(args)
        .current_dir(folder)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("couldn't run `gh`: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "`gh pr {}` failed: {}",
            args[0].to_string_lossy(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// A draft pull request that `open_draft` opened or found.
pub struct Draft {
    pub address: String,
    /// If this call opened it. When it is not new, GitHub opened it in a
    /// call before whose answer got lost, and its description can be an
    /// older one.
    pub new: bool,
}

/// Opens a draft pull request for the branch `head` of the repository in
/// `folder`, into the branch `base`. The description is the text of the
/// file `description`.
pub fn open_draft(
    folder: &Path,
    base: &str,
    head: &str,
    title: &str,
    description: &Path,
) -> Result<Draft, String> {
    let args = ["create", "--draft", "--base", base, "--head", head];
    let mut args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
    args.extend([OsStr::new("--title"), OsStr::new(title)]);
    args.extend([OsStr::new("--body-file"), description.as_os_str()]);
    let printed = match pull_request(folder, &args) {
        Ok(printed) => printed,
        // GitHub can open the pull request, and its answer can get lost.
        // The branch is new and Lira's own, so an open pull request for it
        // is this one.
        Err(error) => {
            let found = open_for(folder, head).ok().flatten().ok_or(error)?;
            return Ok(Draft {
                address: found,
                new: false,
            });
        }
    };
    // `gh` prints the address of the new pull request on the last line.
    match address_in(&printed) {
        Some(address) => Ok(Draft { address, new: true }),
        None => Err(format!(
            "`gh pr create` did not print the address of the pull request. It printed:\n{}",
            printed.trim()
        )),
    }
}

/// The last line of what `gh` printed, when it is an address.
fn address_in(printed: &str) -> Option<String> {
    let last = printed
        .lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())?;
    (last.starts_with("https://") || last.starts_with("http://")).then(|| last.to_string())
}

/// The address of the open pull request for the branch `head`, when there
/// is one.
fn open_for(folder: &Path, head: &str) -> Result<Option<String>, String> {
    let args = ["list", "--head", head, "--state", "open"];
    let mut args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
    // A copy of the repository on GitHub can have an open pull request from
    // a branch with the same name. Lira takes only one of this repository.
    let same = ".[] | select(.isCrossRepository | not) | .url";
    args.extend(["--json", "url,isCrossRepository", "--jq", same].map(OsStr::new));
    Ok(address_in(&pull_request(folder, &args)?))
}

/// Gives the pull request at `address` the text of the file `description`
/// as its description.
pub fn describe(folder: &Path, address: &str, description: &Path) -> Result<(), String> {
    let args = [
        OsStr::new("edit"),
        OsStr::new(address),
        OsStr::new("--body-file"),
        description.as_os_str(),
    ];
    pull_request(folder, &args).map(|_| ())
}

/// Writes the text of the file `text` as a comment on the pull request at
/// `address`.
pub fn comment(folder: &Path, address: &str, text: &Path) -> Result<(), String> {
    let args = [
        OsStr::new("comment"),
        OsStr::new(address),
        OsStr::new("--body-file"),
        text.as_os_str(),
    ];
    pull_request(folder, &args).map(|_| ())
}

/// Adds the label `label` to the pull request at `address`. GitHub makes
/// the label when the repository does not have it yet.
pub fn add_label(folder: &Path, address: &str, label: &str) -> Result<(), String> {
    let number = address
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or_default();
    let labels = format!("repos/{{owner}}/{{repo}}/issues/{number}/labels");
    let label = format!("labels[]={label}");
    api(folder, &[&labels, "--method", "POST", "-f", &label]).map(|_| ())
}

/// Marks the pull request at `address` ready for review.
pub fn mark_ready(folder: &Path, address: &str) -> Result<(), String> {
    pull_request(folder, &[OsStr::new("ready"), OsStr::new(address)]).map(|_| ())
}

/// The scores of the main branch: those in the comment on the last merged
/// pull request. `branch` is the name of the main branch on GitHub. With no
/// such pull request, or no such comment on it, no score has a value.
pub fn main_scores(folder: &Path, branch: &str) -> Result<Scores, String> {
    require_login(folder)?;
    let Some(number) = last_merged(folder, branch)? else {
        return Ok(Scores::default());
    };
    let comments = format!("repos/{{owner}}/{{repo}}/issues/{number}/comments");
    let each = format!(".[] | .body + \"{END_OF_COMMENT}\"");
    let bodies = api(folder, &[&comments, "--paginate", "--jq", &each])?;
    scores_comment(&bodies)
        .map_err(|e| format!("the scores in a comment on pull request #{number}, {e}"))
}

/// The number of the last pull request that was merged into `branch`: the
/// one that the newest commit of the branch came from. A commit that was
/// pushed with no pull request has none, so Lira goes back through the
/// commits until one has. It stops at once when no pull request was merged
/// into the branch.
fn last_merged(folder: &Path, branch: &str) -> Result<Option<String>, String> {
    let mut after = String::new();
    loop {
        let branch_field = format!("branch={branch}");
        let ref_field = format!("ref=refs/heads/{branch}");
        let after_field = format!("after={after}");
        let query = format!("query={HISTORY}");
        let mut args = vec!["graphql", "-F", "owner={owner}", "-F", "name={repo}"];
        args.extend(["-f", &branch_field, "-f", &ref_field]);
        if !after.is_empty() {
            args.extend(["-f", &after_field]);
        }
        args.extend(["-f", &query, "--jq", HISTORY_LINES]);
        let page = api(folder, &args)?;
        let line = |start: &str| page.lines().find_map(|line| line.strip_prefix(start));
        if line("count ") == Some("0") {
            return Ok(None);
        }
        if let Some(number) = merged_into(&page, branch) {
            return Ok(Some(number));
        }
        match line("next ") {
            Some(next) if !next.is_empty() => after = next.to_string(),
            _ => return Ok(None),
        }
    }
}

/// Picks the pull request of the newest commit that came from one which was
/// merged into `branch`. A line is
/// `pull <place of its commit> <number> <time of the merge, or open> <branch>`,
/// and place 0 is the newest commit.
fn merged_into(page: &str, branch: &str) -> Option<String> {
    let merged = page.lines().filter_map(|line| {
        let mut parts = line.strip_prefix("pull ")?.splitn(4, ' ');
        let place: usize = parts.next()?.parse().ok()?;
        let (number, time, into) = (parts.next()?, parts.next()?, parts.next()?);
        (time != "open" && into == branch).then_some((place, time, number))
    });
    // The newest commit first. If it came from two pull requests, the one
    // that was merged later.
    let newest = merged.min_by(|one, other| one.0.cmp(&other.0).then(other.1.cmp(one.1)));
    newest.map(|(_, _, number)| number.to_string())
}

/// The scores in the newest comment that is only score lines.
fn scores_comment(bodies: &str) -> Result<Scores, String> {
    let mut comments = bodies.split(END_OF_COMMENT).rev();
    let newest = comments.find(|body| Scores::are_in(body));
    newest.map_or(Ok(Scores::default()), Scores::read)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scores_are_in_the_newest_comment_that_is_only_score_lines() {
        let bodies = "spec = 1\n\u{1e}\nGood work. spec = 5 now.\n\u{1e}\nspec = 14\r\nsize = 2\n\u{1e}\nThanks!\n\u{1e}\n";
        let scores = scores_comment(bodies).unwrap();
        assert_eq!(scores.names().collect::<Vec<_>>(), ["spec", "size"]);
        assert_eq!(scores.get("spec").unwrap().to_string(), "14");
    }

    #[test]
    fn with_no_such_comment_no_score_has_a_value() {
        assert!(scores_comment("").unwrap().is_empty());
        assert!(scores_comment("Thanks!\n\u{1e}\n").unwrap().is_empty());
    }

    #[test]
    fn a_scores_comment_with_one_score_two_times_is_an_error() {
        let error = scores_comment("s = 40\n\u{1e}\ns = 20\ns = 30\n\u{1e}\n").unwrap_err();
        assert!(
            error.contains("the score `s` is there two times"),
            "{error}"
        );
    }

    #[test]
    fn the_last_merged_pull_request_is_the_one_of_the_newest_commit() {
        let page = "count 3\nnext \npull 1 5 open main\npull 1 8 2026-11-21T10:00:00Z release/1\npull 2 4 2026-11-22T10:00:00Z main\npull 1 6 2026-11-18T10:00:00Z main\npull 1 7 2026-11-20T10:00:00Z main\n";
        assert_eq!(merged_into(page, "main"), Some("7".to_string()));
        assert_eq!(merged_into(page, "release/1"), Some("8".to_string()));
        assert_eq!(
            merged_into("count 0\nnext \npull 0 5 open main\n", "main"),
            None
        );
        assert_eq!(merged_into("", "main"), None);
    }
}
