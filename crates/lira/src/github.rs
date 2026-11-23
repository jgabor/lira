//! What Lira asks GitHub, through the GitHub command line, `gh`. Lira has no
//! token of its own: `gh` uses its login.

use crate::comment::Comment;
use crate::json::{self, Json};
use crate::scores::Scores;
use std::ffi::OsStr;
use std::io::{ErrorKind, Write};
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
    run_pr(folder, args, None)
}

/// Runs `gh pr` in `folder` with these arguments and `--body-file -`, and
/// gives it `text` on its input. Lira writes the text to no file, so that
/// the agent can't change it on its way to GitHub.
fn pull_request_with_text(folder: &Path, args: &[&OsStr], text: &str) -> Result<String, String> {
    let mut args = args.to_vec();
    args.extend([OsStr::new("--body-file"), OsStr::new("-")]);
    run_pr(folder, &args, Some(text))
}

/// Runs `gh pr` in `folder` with these arguments, and gives it `text` on its
/// input when there is a text.
fn run_pr(folder: &Path, args: &[&OsStr], text: Option<&str>) -> Result<String, String> {
    let mut child = Command::new("gh")
        .arg("pr")
        .args(args)
        .current_dir(folder)
        .stdin(match text {
            Some(_) => Stdio::piped(),
            None => Stdio::null(),
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("couldn't run `gh`: {e}"))?;
    // A thread writes the text, so that `gh` can write its output while it
    // reads. The input ends when the thread drops it.
    let writer = child.stdin.take().zip(text).map(|(mut input, text)| {
        let text = text.to_string();
        std::thread::spawn(move || input.write_all(text.as_bytes()))
    });
    let output = child
        .wait_with_output()
        .map_err(|e| format!("couldn't run `gh`: {e}"))?;
    let written = writer.map(|writer| writer.join());
    if !output.status.success() {
        return Err(format!(
            "`gh pr {}` failed: {}",
            args[0].to_string_lossy(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    if let Some(Ok(Err(error))) = written {
        return Err(format!(
            "Lira couldn't give the text to `gh pr {}`: {error}",
            args[0].to_string_lossy()
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
/// `folder`, into the branch `base`, with this description.
pub fn open_draft(
    folder: &Path,
    base: &str,
    head: &str,
    title: &str,
    description: &str,
) -> Result<Draft, String> {
    let args = ["create", "--draft", "--base", base, "--head", head];
    let mut args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
    args.extend([OsStr::new("--title"), OsStr::new(title)]);
    let printed = match pull_request_with_text(folder, &args, description) {
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

/// Gives the pull request at `address` this description.
pub fn describe(folder: &Path, address: &str, description: &str) -> Result<(), String> {
    let args = [OsStr::new("edit"), OsStr::new(address)];
    pull_request_with_text(folder, &args, description).map(|_| ())
}

/// Writes `text` as a comment on the pull request at `address`.
pub fn comment(folder: &Path, address: &str, text: &str) -> Result<(), String> {
    let args = [OsStr::new("comment"), OsStr::new(address)];
    pull_request_with_text(folder, &args, text).map(|_| ())
}

/// Adds the label `label` to the pull request at `address`. GitHub makes
/// the label when the repository does not have it yet.
pub fn add_label(folder: &Path, address: &str, label: &str) -> Result<(), String> {
    let labels = format!("{}/labels", issue_of(address));
    let label = format!("labels[]={label}");
    api(folder, &[&labels, "--method", "POST", "-f", &label]).map(|_| ())
}

/// Removes the label `label` from the pull request at `address`.
pub fn remove_label(folder: &Path, address: &str, label: &str) -> Result<(), String> {
    let label = format!("{}/labels/{label}", issue_of(address));
    api(folder, &[&label, "--method", "DELETE"]).map(|_| ())
}

/// The path of the API for the issue of the pull request at `address`. On
/// GitHub, each pull request is an issue too, with the same number.
fn issue_of(address: &str) -> String {
    let number = address
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or_default();
    format!("repos/{{owner}}/{{repo}}/issues/{number}")
}

/// The path of the API for the pull request at `address`.
fn pull_of(address: &str) -> String {
    issue_of(address).replace("/issues/", "/pulls/")
}

/// Makes the pull request at `address` a draft again.
pub fn mark_draft(folder: &Path, address: &str) -> Result<(), String> {
    let args = [
        OsStr::new("ready"),
        OsStr::new(address),
        OsStr::new("--undo"),
    ];
    pull_request(folder, &args).map(|_| ())
}

/// The login of `gh`.
pub fn login(folder: &Path) -> Result<String, String> {
    let login = api(folder, &["user", "--jq", ".login"])?;
    match login.trim() {
        "" => Err("`gh api user` printed no login".to_string()),
        login => Ok(login.to_string()),
    }
}

/// What GitHub says about a pull request.
pub struct View {
    /// Its state, such as `OPEN`, `CLOSED` or `MERGED`.
    pub state: String,
    pub draft: bool,
    /// If its branch is in another repository, such as a fork.
    pub cross: bool,
    pub address: String,
    /// The name of its branch.
    pub branch: String,
    pub labels: Vec<String>,
}

/// Prints what Lira reads of a pull request, one thing on each line.
const VIEW_LINES: &str = r#""state \(.state)", "draft \(.isDraft)", "cross \(.isCrossRepository)", "address \(.url)", "branch \(.headRefName)", (.labels[] | "label \(.name)")"#;

/// What GitHub says about the pull request with this number, in the
/// repository of `folder`.
pub fn view(folder: &Path, number: &str) -> Result<View, String> {
    let fields = "state,isDraft,isCrossRepository,url,headRefName,labels";
    let args = ["view", number, "--json", fields, "--jq", VIEW_LINES].map(OsStr::new);
    let printed = pull_request(folder, &args)?;
    let line = |start: &str| {
        printed
            .lines()
            .find_map(|line| line.strip_prefix(start))
            .map(str::to_string)
    };
    let missing = || {
        format!(
            "`gh pr view {number}` did not print all that Lira reads. It printed:\n{}",
            printed.trim()
        )
    };
    Ok(View {
        state: line("state ").ok_or_else(missing)?,
        draft: line("draft ").ok_or_else(missing)? == "true",
        cross: line("cross ").ok_or_else(missing)? == "true",
        address: line("address ").ok_or_else(missing)?,
        branch: line("branch ").ok_or_else(missing)?,
        labels: printed
            .lines()
            .filter_map(|line| line.strip_prefix("label "))
            .map(str::to_string)
            .collect(),
    })
}

/// The comments on the pull request at `address`, the oldest first: the
/// comments of the conversation, the reviews with a text, and the comments
/// on a line of the change.
pub fn comments(folder: &Path, address: &str) -> Result<Vec<Comment>, String> {
    let pull = pull_of(address);
    // Each comment is one line of JSON, so no text of a comment can look
    // like the start of another one.
    // GitHub has no user for a comment of a login that is gone, and shows
    // `ghost` for it.
    let conversation = ".[] | [.created_at, .user.login // \"ghost\", .body] | @json";
    // A review that is not sent yet has no time.
    let reviews = ".[] | [.submitted_at, .user.login // \"ghost\", .body, .id] | @json";
    let lines = ".[] | {created_at, login: (.user.login // \"ghost\"), body, path, line, start_line, original_line, original_start_line, side, pull_request_review_id} | @json";
    let mut all = Vec::new();
    for (path, each, source) in [
        (
            format!("{}/comments", issue_of(address)),
            conversation,
            Source::Conversation,
        ),
        (format!("{pull}/reviews"), reviews, Source::Reviews),
        (format!("{pull}/comments"), lines, Source::Lines),
    ] {
        let printed = api(folder, &[&path, "--paginate", "--jq", each])?;
        all.extend(comments_in(&printed, source)?);
    }
    Ok(in_order(all))
}

/// Where on GitHub a list of comments comes from.
#[derive(Debug, Clone, Copy)]
enum Source {
    /// The comments of the conversation. On each line `gh --jq` prints a
    /// list of JSON with the time, the login and the text of one comment.
    Conversation,
    /// The reviews. A line has the number of the review after the text.
    Reviews,
    /// The comments on a line. A line is an object of JSON with the fields
    /// of GitHub, and `login` for the login.
    Lines,
}

/// One comment as `gh --jq` prints it.
#[derive(Debug, PartialEq, Eq)]
struct Listed {
    /// When it was written. A review that is not sent yet has none.
    time: Option<String>,
    comment: Comment,
    kind: Kind,
}

/// What a comment is.
#[derive(Debug, PartialEq, Eq)]
enum Kind {
    Conversation,
    /// A review, with its number.
    Review {
        id: i64,
    },
    /// A comment on lines of a file.
    Line {
        path: String,
        /// The first and the last line. A comment on the whole file has
        /// none.
        lines: Option<(i64, i64)>,
        /// If the lines are in an older version of the file: the file
        /// before the change, or a commit before the newest one.
        older: bool,
        /// The number of the review that the comment is part of.
        review: Option<i64>,
    },
}

/// The comments in what `gh --jq` printed for `source`.
fn comments_in(printed: &str, source: Source) -> Result<Vec<Listed>, String> {
    printed
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let wrong = || format!("`gh api` printed a line that is no comment: {line}");
            let value = json::read(line).map_err(|_| wrong())?;
            let text = |value: &Json| value.as_text().map(str::to_string).ok_or_else(wrong);
            let number = |value: Option<&Json>| match value {
                None | Some(Json::Null) => Ok(None),
                Some(value) => value.as_i64().map(Some).ok_or_else(wrong),
            };
            // A comment with no text has `null` there.
            let comment = |author: &Json, body: Option<&Json>| {
                Ok::<_, String>(Comment {
                    author: text(author)?,
                    body: body.and_then(Json::as_text).unwrap_or_default().to_string(),
                })
            };
            match (source, value.as_list()) {
                (Source::Conversation, Some([time, author, body])) => Ok(Listed {
                    time: Some(text(time)?),
                    comment: comment(author, Some(body))?,
                    kind: Kind::Conversation,
                }),
                (Source::Reviews, Some([time, author, body, id])) => Ok(Listed {
                    time: match time {
                        Json::Null => None,
                        time => Some(text(time)?),
                    },
                    comment: comment(author, Some(body))?,
                    kind: Kind::Review {
                        id: number(Some(id))?.ok_or_else(wrong)?,
                    },
                }),
                (Source::Lines, None) => {
                    let field = |key: &str| value.get(key);
                    let line = |key: &str| number(field(key));
                    // GitHub gives `line` on the newest version of the file,
                    // and only `original_line` when that version no longer
                    // has it. A comment on the whole file has neither.
                    let (lines, older) = match (line("line")?, line("original_line")?) {
                        (Some(end), _) => (
                            Some((line("start_line")?.unwrap_or(end), end)),
                            field("side").and_then(Json::as_text) == Some("LEFT"),
                        ),
                        (None, Some(end)) => (
                            Some((line("original_start_line")?.unwrap_or(end), end)),
                            true,
                        ),
                        (None, None) => (None, false),
                    };
                    Ok(Listed {
                        time: Some(text(field("created_at").ok_or_else(wrong)?)?),
                        comment: comment(field("login").ok_or_else(wrong)?, field("body"))?,
                        kind: Kind::Line {
                            path: text(field("path").ok_or_else(wrong)?)?,
                            lines,
                            older,
                            review: line("pull_request_review_id")?,
                        },
                    })
                }
                _ => Err(wrong()),
            }
        })
        .collect()
}

/// The comments in the order of their time, with the oldest first. Comments
/// with the same time keep their order. A comment on a line that is part of
/// a review has the time when the review was sent. A review that is not
/// sent yet is no comment, and neither are its comments on a line. A review
/// with no text is no comment. The text of a review and of a comment on a
/// line starts with what it is.
fn in_order(all: Vec<Listed>) -> Vec<Comment> {
    let sent: Vec<(i64, Option<String>)> = all
        .iter()
        .filter_map(|listed| match listed.kind {
            Kind::Review { id } => Some((id, listed.time.clone())),
            _ => None,
        })
        .collect();
    let mut kept: Vec<(String, Listed)> = all
        .into_iter()
        .filter_map(|listed| {
            let time = match &listed.kind {
                Kind::Review { .. } if listed.comment.body.trim().is_empty() => return None,
                Kind::Line {
                    review: Some(review),
                    ..
                } => match sent.iter().find(|(id, _)| id == review) {
                    Some((_, time)) => time.clone(),
                    None => listed.time.clone(),
                },
                _ => listed.time.clone(),
            };
            Some((time?, listed))
        })
        .collect();
    kept.sort_by(|one, other| one.0.cmp(&other.0));
    kept.into_iter()
        .map(|(_, Listed { comment, kind, .. })| {
            let start = match kind {
                Kind::Conversation => return comment,
                Kind::Review { .. } => "In a review:".to_string(),
                Kind::Line {
                    path, lines, older, ..
                } => {
                    let place = match lines {
                        None => String::new(),
                        Some((start, end)) if start != end => {
                            format!("lines {start} to {end} of ")
                        }
                        Some((_, end)) => format!("line {end} of "),
                    };
                    let version = if older { "an older version of " } else { "" };
                    format!("On {place}{version}`{path}`:")
                }
            };
            Comment {
                body: format!("{start}\n\n{}", comment.body.trim()),
                ..comment
            }
        })
        .collect()
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

    fn of(author: &str, body: &str) -> Comment {
        Comment {
            author: author.to_string(),
            body: body.to_string(),
        }
    }

    fn listed(time: Option<&str>, body: &str, kind: Kind) -> Listed {
        Listed {
            time: time.map(str::to_string),
            comment: of("you", body),
            kind,
        }
    }

    fn on(path: &str, lines: Option<(i64, i64)>, older: bool, review: Option<i64>) -> Kind {
        Kind::Line {
            path: path.to_string(),
            lines,
            older,
            review,
        }
    }

    #[test]
    fn each_comment_has_its_time_its_login_and_its_text() {
        let printed = "[\"t1\",\"you\",\"One.\\n\\nTwo.\"]\n[\"t2\",\"someone\",\"a\\u001e[\\\"t3\\\",\\\"you\\\",\\\"b\\\"]\"]\n[\"t4\",\"other\",null]\n";
        let listed = |time: &str, comment: Comment| Listed {
            time: Some(time.to_string()),
            comment,
            kind: Kind::Conversation,
        };
        assert_eq!(
            comments_in(printed, Source::Conversation).unwrap(),
            [
                listed("t1", of("you", "One.\n\nTwo.")),
                // Text in a comment that looks like another comment is text.
                listed("t2", of("someone", "a\u{1e}[\"t3\",\"you\",\"b\"]")),
                listed("t4", of("other", ""))
            ]
        );
        assert_eq!(comments_in("", Source::Conversation).unwrap(), []);
        for wrong in [
            "you\u{1f}One.\u{1e}\n",
            "[\"you\",\"One.\"]\n",
            // Only a review that is not sent yet has no time.
            "[null,\"you\",\"One.\"]\n",
        ] {
            assert!(comments_in(wrong, Source::Conversation).is_err(), "{wrong}");
        }
    }

    #[test]
    fn a_review_has_its_number_and_can_have_no_time() {
        let printed = "[null,\"you\",\"Not sent.\",4]\n[\"t1\",\"you\",null,5]\n";
        assert_eq!(
            comments_in(printed, Source::Reviews).unwrap(),
            [
                listed(None, "Not sent.", Kind::Review { id: 4 }),
                listed(Some("t1"), "", Kind::Review { id: 5 }),
            ]
        );
        assert!(comments_in("[\"t1\",\"you\",\"One.\"]\n", Source::Reviews).is_err());
        assert!(comments_in("[\"t1\",\"you\",\"One.\",null]\n", Source::Reviews).is_err());
    }

    #[test]
    fn a_comment_on_a_line_has_its_file_its_lines_and_its_review() {
        let printed = concat!(
            r#"{"created_at":"t1","login":"you","body":"Here.","path":"a.rs","line":12,"start_line":null,"original_line":9,"original_start_line":null,"side":"RIGHT","pull_request_review_id":5}"#,
            "\n",
            r#"{"created_at":"t2","login":"you","body":"These.","path":"a.rs","line":12,"start_line":10,"side":"RIGHT","pull_request_review_id":null}"#,
            "\n",
            // A line that the change removes.
            r#"{"created_at":"t3","login":"you","body":"Gone.","path":"a.rs","line":3,"side":"LEFT"}"#,
            "\n",
            // A line that the newest version of the file no longer has.
            r#"{"created_at":"t4","login":"you","body":"Old.","path":"a.rs","line":null,"start_line":null,"original_line":8,"original_start_line":7,"side":"RIGHT"}"#,
            "\n",
            // A comment on the whole file.
            r#"{"created_at":"t5","login":"you","body":"File.","path":"b.rs","line":null,"original_line":null}"#,
            "\n",
        );
        assert_eq!(
            comments_in(printed, Source::Lines).unwrap(),
            [
                listed(
                    Some("t1"),
                    "Here.",
                    on("a.rs", Some((12, 12)), false, Some(5))
                ),
                listed(
                    Some("t2"),
                    "These.",
                    on("a.rs", Some((10, 12)), false, None)
                ),
                listed(Some("t3"), "Gone.", on("a.rs", Some((3, 3)), true, None)),
                listed(Some("t4"), "Old.", on("a.rs", Some((7, 8)), true, None)),
                listed(Some("t5"), "File.", on("b.rs", None, false, None)),
            ]
        );
        for wrong in [
            r#"{"created_at":"t1","login":"you","body":"Here.","path":"a.rs","line":"12"}"#,
            r#"{"created_at":null,"login":"you","body":"Here.","path":"a.rs"}"#,
            r#"{"created_at":"t1","login":"you","body":"Here."}"#,
            "[\"t1\",\"you\",\"Here.\"]",
        ] {
            assert!(comments_in(wrong, Source::Lines).is_err(), "{wrong}");
        }
    }

    #[test]
    fn the_comments_are_in_the_order_of_their_time() {
        let all = vec![
            listed(Some("2026-11-01T10:00:00Z"), "First.", Kind::Conversation),
            listed(Some("2026-11-01T12:00:00Z"), "Fourth.", Kind::Conversation),
            listed(Some("2026-11-01T10:00:00Z"), "Second.", Kind::Conversation),
            listed(None, "Not sent.", Kind::Review { id: 1 }),
            listed(Some("2026-11-01T11:00:00Z"), "  ", Kind::Review { id: 2 }),
            listed(
                Some("2026-11-01T11:00:00Z"),
                "Third.\n",
                Kind::Review { id: 3 },
            ),
            listed(
                Some("2026-11-01T13:00:00Z"),
                "Fifth.",
                on("a.rs", Some((3, 3)), false, None),
            ),
            listed(
                Some("2026-11-01T14:00:00Z"),
                "Sixth.",
                on("a.rs", Some((3, 5)), true, None),
            ),
            listed(
                Some("2026-11-01T15:00:00Z"),
                "Seventh.",
                on("a.rs", None, false, None),
            ),
            // Comments of reviews: they have the time of their review, also
            // when they were written before it.
            listed(
                Some("2026-11-01T09:00:00Z"),
                "In the review with no text.",
                on("b.rs", Some((1, 1)), false, Some(2)),
            ),
            listed(
                Some("2026-11-01T09:00:00Z"),
                "In the review not sent.",
                on("b.rs", Some((1, 1)), false, Some(1)),
            ),
            // A review that GitHub did not list.
            listed(
                Some("2026-11-01T09:30:00Z"),
                "In another review.",
                on("b.rs", Some((2, 2)), false, Some(9)),
            ),
        ];
        assert_eq!(
            in_order(all),
            [
                of("you", "On line 2 of `b.rs`:\n\nIn another review."),
                of("you", "First."),
                of("you", "Second."),
                of("you", "In a review:\n\nThird."),
                of("you", "On line 1 of `b.rs`:\n\nIn the review with no text."),
                of("you", "Fourth."),
                of("you", "On line 3 of `a.rs`:\n\nFifth."),
                of(
                    "you",
                    "On lines 3 to 5 of an older version of `a.rs`:\n\nSixth."
                ),
                of("you", "On `a.rs`:\n\nSeventh."),
            ]
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
