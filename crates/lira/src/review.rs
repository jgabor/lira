//! The Reviewer: an agent that reads a change, and says if it is good.

use crate::sandbox::Sandbox;
use crate::worktree::Worktree;
use crate::{agent, git};
use std::fs;
use std::path::{Path, PathBuf};

/// The instructions for the Reviewer. Lira puts the request and the change
/// in the places with a name in braces.
const INSTRUCTIONS: &str = include_str!("reviewer.md");

/// The longest change that Lira puts in the message for the Reviewer, in
/// bytes. For a longer one, the Reviewer gets the names of the files, and
/// reads the change itself.
const LONGEST: usize = 200_000;

/// What a Reviewer said about a change.
pub struct Review {
    /// If the Reviewer says that the agent has nothing to repair.
    pub passed: bool,
    /// All that the Reviewer wrote.
    pub text: String,
}

/// A Reviewer reads the change that the worktree has at the commit `to`.
/// `from` is the commit of the main branch that the change is compared
/// with, and `started` is the folder that Lira started in.
///
/// The Reviewer can't change a file of the change. It works in a copy of
/// the repository for itself alone, which shares no branch with the
/// repository and knows no remote, and Lira removes the copy afterwards.
/// Its sandbox holds it to the copy, to a data folder of its harness for
/// itself alone, and to the login of the harness. Lira also says no to each
/// of its permission requests.
pub fn run(
    started: &Path,
    worktree: &Worktree,
    request: &str,
    from: &str,
    to: &str,
) -> Result<Review, String> {
    let message = message(worktree, request, from, to)?;
    let copy = copy_folder(&worktree.folder);
    // The folder must be new: Lira removes it with all that is in it.
    fs::create_dir(&copy).map_err(|error| {
        format!(
            "Lira can't make the folder `{}` for the Reviewer: {error}",
            copy.display()
        )
    })?;
    let read = git::add_copy(&worktree.folder, &copy, to)
        .and_then(|()| read(started, worktree, &copy, &message));
    // The copy goes away, also when the Reviewer failed.
    let removed = fs::remove_dir_all(&copy).map_err(|error| {
        format!(
            "Lira can't remove the folder `{}` of the Reviewer: {error}",
            copy.display()
        )
    });
    let text = read?;
    removed?;
    Ok(Review {
        passed: passes(&text),
        text,
    })
}

/// Starts a Reviewer in the copy, gives it the message, and returns what
/// it wrote.
fn read(started: &Path, worktree: &Worktree, copy: &Path, message: &str) -> Result<String, String> {
    let model = worktree.file.models.reviewer.as_deref();
    // A Reviewer uses a different harness from the agent when the Lira file
    // names one that is available. Lira knows one harness now, so the
    // Reviewer has the harness of the agent.
    // The copy is the only folder of the repository that the Reviewer can
    // change, and Lira removes it afterwards.
    let mut sandbox = Sandbox::new();
    sandbox.writable(copy);
    let mut agent = agent::start(started, copy, &mut sandbox, &worktree.file, model)
        .map_err(|error| format!("the Reviewer did not start: {error}"))?;
    let reply = agent
        .harness
        .send(&agent.conversation, message, &mut |_| false)
        .map_err(|error| format!("the Reviewer did not end its review: {error}"))?;
    drop(agent);
    match agent::stopped(&reply.stop) {
        None => Ok(reply.text),
        Some(why) => Err(format!("the Reviewer did not end its review: {why}")),
    }
}

/// The folder for the copy of a Reviewer: beside the worktree of the
/// change, with `-review` after its name.
fn copy_folder(worktree: &Path) -> PathBuf {
    let mut name = worktree.file_name().unwrap_or_default().to_os_string();
    name.push("-review");
    worktree.with_file_name(name)
}

/// The message for the Reviewer: its instructions, with the request, the
/// commits and the change in their places.
fn message(worktree: &Worktree, request: &str, from: &str, to: &str) -> Result<String, String> {
    let folder = &worktree.folder;
    let commits: String = git::subjects(folder, from, to)?
        .iter()
        .map(|subject| format!("- {subject}\n"))
        .collect();
    let mut change = git::diff(folder, from, to, false)?;
    if change.len() > LONGEST {
        change = format!(
            "The change is too long for this message. Read it with `git diff {from}...{to}`, or read the files that it names:\n\n{}",
            git::diff(folder, from, to, true)?
        );
    }
    Ok(fill(
        INSTRUCTIONS,
        &[
            ("request", request),
            ("commits", commits.trim_end()),
            ("from", from),
            ("to", to),
            ("change", change.trim_end()),
        ],
    ))
}

/// Puts each value in the place of its name in braces. A value is not read
/// again, so braces in a request or in a change stay as they are.
pub fn fill(text: &str, values: &[(&str, &str)]) -> String {
    let mut filled = String::new();
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        filled.push_str(&rest[..open]);
        rest = &rest[open..];
        let value = values.iter().find_map(|(name, value)| {
            let place = format!("{{{name}}}");
            rest.starts_with(&place).then_some((place.len(), value))
        });
        match value {
            Some((length, value)) => {
                filled.push_str(value);
                rest = &rest[length..];
            }
            None => {
                filled.push('{');
                rest = &rest[1..];
            }
        }
    }
    filled + rest
}

/// If the last line of what a Reviewer wrote is `Review: pass`. All other
/// endings mean that the agent has something to repair: a Reviewer that
/// does not say that a change is good did not pass it.
fn passes(text: &str) -> bool {
    let last = text.lines().rev().find(|line| !line.trim().is_empty());
    let marks = |c: char| c.is_whitespace() || matches!(c, '`' | '*' | '_' | '.');
    last.is_some_and(|line| {
        line.trim_matches(marks)
            .eq_ignore_ascii_case("review: pass")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_line_of_a_review_says_if_the_change_passed() {
        assert!(passes("Review: pass"));
        assert!(passes("All good.\n\nReview: pass\n\n"));
        assert!(passes("Good.\n**Review: pass**"));
        assert!(passes("Good.\n`review: PASS`."));
        assert!(!passes("Review: findings"));
        assert!(!passes("Review: pass\nBut one more thing."));
        assert!(!passes("I would say Review: pass"));
        assert!(!passes("Review: pass, with findings"));
        assert!(!passes("Good."));
        assert!(!passes(""));
    }

    #[test]
    fn a_value_is_put_in_its_place_and_is_not_read_again() {
        let values = [
            ("request", "Use {change} and {x}"),
            ("change", "+ a {request}"),
        ];
        assert_eq!(
            fill("R: {request}\nC: {change}\n{other} {", &values),
            "R: Use {change} and {x}\nC: + a {request}\n{other} {"
        );
    }
}
