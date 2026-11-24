//! The comments of Lira on a pull request, and what `lira continue` reads
//! from all comments there.
//!
//! Lira writes with your login, so a comment of Lira and a comment of yours
//! have the same author. The first line of a comment of Lira tells them
//! apart. GitHub does not show that line.

/// The first line of the comment that has the request.
const REQUEST: &str = "<!-- lira request -->";

/// The first line of a comment that says what stops the work.
const QUESTION: &str = "<!-- lira question -->";

/// The first line of the comment that says that the work of `lira
/// continue` passed.
const PASSED: &str = "<!-- lira passed -->";

/// The start and the end of the line, in the comment with the request,
/// that names the branch that the worktree started on. GitHub does not
/// show that line.
const STARTED: (&str, &str) = ("<!-- lira started on ", " -->");

/// The start and the end of the line, in a question and in the comment
/// that the work passed, that names what the agent read. GitHub does not
/// show that line.
const READ: (&str, &str) = ("<!-- lira read until ", " -->");

/// What is between the comments and their time in the line `READ`.
const AT: &str = " at ";

/// What is between two comments in the line `READ`.
const AND: &str = ", ";

/// What the line `READ` names when the agent read only the request.
const THE_REQUEST: &str = "the request";

/// One comment on a pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    /// What names the comment on GitHub, such as `comment 12`, when GitHub
    /// gives it.
    pub id: Option<String>,
    /// When GitHub says that it was written, such as
    /// `2026-11-01T10:00:00Z`, when GitHub gives it.
    pub time: Option<String>,
    /// The login that wrote it.
    pub author: String,
    pub body: String,
}

/// What the agent read before Lira wrote a question, or the comment that
/// the work passed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Read {
    /// The request only: the agent of `lira run` reads no comment.
    Request,
    /// Each comment until the last one that `ids` names: the comments
    /// that were there when the agent of `lira continue` started. `time`
    /// is the time of the newest comment, so that Lira finds the place also
    /// when the comment is gone. GitHub gives a time in seconds, so `ids`
    /// names each comment of that second that the agent read, the newest
    /// last. A comment of that second that `ids` does not name is newer.
    Until {
        ids: Vec<String>,
        time: Option<String>,
    },
}

/// The line that says what the agent read.
fn read_line(read: Option<&Read>) -> String {
    let (start, end) = READ;
    match read {
        None => String::new(),
        Some(Read::Request) => format!("{start}{THE_REQUEST}{end}\n"),
        Some(Read::Until { ids, time: None }) => format!("{start}{}{end}\n", ids.join(AND)),
        Some(Read::Until {
            ids,
            time: Some(time),
        }) => format!("{start}{}{AT}{time}{end}\n", ids.join(AND)),
    }
}

/// The comment that keeps the request on the pull request. `branch` is
/// the branch that the worktree started on. The folder of the worktree has
/// its name, also when the agent gives the branch another name.
pub fn request(request: &str, branch: &str) -> String {
    let (start, end) = STARTED;
    format!(
        "{REQUEST}\n{start}{branch}{end}\nThe request that the agent works on:\n\n{}",
        quote(request)
    )
}

/// The most characters that GitHub takes in a comment.
pub const LONGEST: usize = 65_536;

/// If the comment with the request fits in a comment on GitHub.
pub fn request_fits(request: &str, branch: &str) -> bool {
    self::request(request, branch).chars().count() <= LONGEST
}

/// A text as a quote of Markdown.
pub fn quote(text: &str) -> String {
    text.trim_end()
        .lines()
        .map(|line| match line {
            "" => ">\n".to_string(),
            line => format!("> {line}\n"),
        })
        .collect()
}

/// The comment that says what stops the work. `read` is what the agent
/// read. A later `lira continue` reads what you wrote after that.
pub fn question(question: &str, read: Option<&Read>) -> String {
    format!("{QUESTION}\n{}{question}", read_line(read))
}

/// The comment that says that the check and a Reviewer passed the change
/// of `lira continue`. `read` is what the agent read. A later `lira
/// continue` reads only what you wrote after that.
pub fn passed(read: Option<&Read>) -> String {
    format!(
        "{PASSED}\n{}The check and a Reviewer passed the change, and the pull request is ready for review.",
        read_line(read)
    )
}

/// What `lira continue` reads from the comments of a pull request.
#[derive(Debug, PartialEq, Eq)]
pub struct Comments {
    /// The request, from the first comment that has it.
    pub request: String,
    /// The branch that the worktree started on, when that comment names
    /// it.
    pub started_on: Option<String>,
    /// What the last comment of Lira says, when it is a question.
    pub stopped: Option<String>,
    /// If the last comment of Lira says that the work passed.
    pub passed: bool,
    /// Your answer: each comment of `login` after what the agent of the
    /// last comment of Lira read, that Lira did not write.
    pub answer: Vec<String>,
    /// Each comment of someone else after what that agent read.
    pub others: Vec<Comment>,
    /// Each comment of `login` after the request and before your answer,
    /// that Lira did not write.
    pub earlier: Vec<String>,
}

/// Reads the request, what stopped the work, and your answer from the
/// comments of a pull request, the oldest first. `login` is the login of
/// `gh`. Only a comment of that login can be a comment of Lira. It is
/// `None` when no comment has the request.
///
/// Your answer starts after what the agent of the last comment of Lira
/// read. So a comment that you wrote while that agent worked is part of
/// it. When the comment that it names is gone, your answer starts after
/// the last comment that is not newer than it. A comment with the time of
/// the newest comment that the agent read, and that the last comment of
/// Lira does not name, is part of your answer too. When the last comment of
/// Lira does not say what its agent read, your answer starts after that
/// comment.
pub fn read(comments: &[Comment], login: &str) -> Option<Comments> {
    let of_lira = |comment: &Comment| comment.author == login && marker(&comment.body).is_some();
    let (first, (request, started_on)) = comments
        .iter()
        .enumerate()
        .filter(|(_, comment)| comment.author == login)
        .find_map(|(at, comment)| Some((at, request_in(&comment.body)?)))?;
    let last = comments.iter().rposition(of_lira)?;
    let (read, said) = match marker(&comments[last].body) {
        Some((QUESTION | PASSED, rest)) => read_in(rest),
        Some((_, rest)) => (None, rest),
        None => (None, ""),
    };
    let named = |comment: &Comment| match &read {
        Some(Read::Until { ids, .. }) => comment.id.as_ref().is_some_and(|id| ids.contains(id)),
        _ => false,
    };
    // The last comment that the agent read. When it is gone, the last one
    // that is not newer than it.
    let until = match &read {
        Some(Read::Request) => first,
        Some(Read::Until { time, .. }) => {
            let before = &comments[..last];
            before
                .iter()
                .rposition(named)
                .or_else(|| {
                    let time = time.as_deref()?;
                    let not_newer =
                        |comment: &Comment| comment.time.as_deref().is_some_and(|at| at <= time);
                    Some(before.iter().rposition(not_newer).unwrap_or(first))
                })
                .unwrap_or(last)
        }
        None => last,
    };
    let until = until.max(first);
    // Comments of one second can be in another order than the one in which
    // they were written. So a comment of the second of the newest one that
    // the agent read, and that the last comment of Lira does not name, is
    // newer, also when it comes before.
    let newest = match &read {
        Some(Read::Until { time, .. }) => time.as_deref(),
        _ => None,
    };
    let unread = |at: usize, comment: &Comment| {
        at > until
            || (at > first
                && newest.is_some()
                && comment.time.as_deref() == newest
                && !named(comment))
    };
    let earlier = comments[..until + 1]
        .iter()
        .enumerate()
        .skip(first + 1)
        .filter(|(at, comment)| {
            !unread(*at, comment) && comment.author == login && !of_lira(comment)
        })
        .map(|(_, comment)| comment.body.trim().to_string())
        .collect();
    let (stopped, passed) = match marker(&comments[last].body) {
        Some((QUESTION, _)) => (Some(said.trim().to_string()), false),
        Some((PASSED, _)) => (None, true),
        _ => (None, false),
    };
    let after: Vec<&Comment> = comments
        .iter()
        .enumerate()
        .filter(|(at, comment)| unread(*at, comment))
        .map(|(_, comment)| comment)
        .collect();
    let answer = after
        .iter()
        .filter(|comment| comment.author == login && !of_lira(comment))
        .map(|comment| comment.body.trim().to_string())
        .collect();
    let others = after
        .iter()
        .filter(|comment| comment.author != login)
        .map(|comment| (*comment).clone())
        .collect();
    Some(Comments {
        request,
        started_on,
        stopped,
        passed,
        answer,
        others,
        earlier,
    })
}

/// The marker on the first line of a comment of Lira, and the text after
/// that line.
fn marker(body: &str) -> Option<(&'static str, &str)> {
    let (first, rest) = body.split_once('\n').unwrap_or((body, ""));
    [REQUEST, QUESTION, PASSED]
        .into_iter()
        .find(|marker| first.trim_end() == *marker)
        .map(|marker| (marker, rest))
}

/// What the line after the marker says that the agent read, when it is
/// that line, and the text after it.
fn read_in(rest: &str) -> (Option<Read>, &str) {
    let (line, after) = rest.split_once('\n').unwrap_or((rest, ""));
    let (start, end) = READ;
    match line
        .trim_end()
        .strip_prefix(start)
        .and_then(|line| line.strip_suffix(end))
    {
        Some(THE_REQUEST) => (Some(Read::Request), after),
        Some(read) if !read.is_empty() => {
            let (ids, time) = match read.rsplit_once(AT) {
                Some((ids, time)) => (ids, Some(time.to_string())),
                None => (read, None),
            };
            (
                Some(Read::Until {
                    ids: ids.split(AND).map(str::to_string).collect(),
                    time,
                }),
                after,
            )
        }
        _ => (None, rest),
    }
}

/// The request in a comment that has it: the lines of its quote. And the
/// branch that the worktree started on, when the line after the marker
/// names it.
fn request_in(body: &str) -> Option<(String, Option<String>)> {
    let (REQUEST, rest) = marker(body)? else {
        return None;
    };
    let (start, end) = STARTED;
    let started_on = rest
        .lines()
        .next()
        .and_then(|line| line.trim_end().strip_prefix(start)?.strip_suffix(end))
        .filter(|branch| !branch.is_empty())
        .map(str::to_string);
    let lines: Vec<&str> = rest
        .lines()
        .skip_while(|line| !line.starts_with('>'))
        .take_while(|line| line.starts_with('>'))
        .map(|line| line.strip_prefix("> ").unwrap_or(&line[1..]))
        .collect();
    let request = lines.join("\n");
    (!request.trim().is_empty()).then_some((request, started_on))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A branch that a worktree started on.
    const BRANCH: &str = "lira/run-1a2b3c4d";

    fn of(author: &str, body: &str) -> Comment {
        Comment {
            id: None,
            time: None,
            author: author.to_string(),
            body: body.to_string(),
        }
    }

    #[test]
    fn a_request_fits_when_its_comment_has_at_most_the_longest_count_of_characters() {
        let around = request("x", BRANCH).chars().count() - 1;
        // Each character here is more than one byte, and counts as one.
        let fits = "é".repeat(LONGEST - around);
        assert_eq!(request(&fits, BRANCH).chars().count(), LONGEST);
        assert!(request_fits(&fits, BRANCH));
        assert!(!request_fits(&format!("{fits}é"), BRANCH));
    }

    #[test]
    fn the_request_comes_back_from_its_comment_as_it_was() {
        for text in [
            "Add a file.",
            "Add a file.\n\n  Make it `short`.\n> Quoted.",
        ] {
            let comments = [of("you", &request(text, BRANCH))];
            assert_eq!(read(&comments, "you").unwrap().request, text);
        }
        // GitHub can give the lines of a comment with a carriage return.
        let body = request("One.\n\nTwo.", BRANCH).replace('\n', "\r\n");
        let read = read(&[of("you", &body)], "you").unwrap();
        assert_eq!(read.request, "One.\n\nTwo.");
        assert_eq!(read.started_on.as_deref(), Some(BRANCH));
    }

    #[test]
    fn the_comment_with_the_request_names_the_branch_that_the_worktree_started_on() {
        let comments = [of("you", &request("Do it.", BRANCH))];
        let read = read(&comments, "you").unwrap();
        assert_eq!(read.started_on.as_deref(), Some(BRANCH));
        // A comment with the request and no such line names no branch.
        let body = format!("{REQUEST}\nThe request that the agent works on:\n\n> Do it.\n");
        let read = super::read(&[of("you", &body)], "you").unwrap();
        assert_eq!(read.request, "Do it.");
        assert_eq!(read.started_on, None);
    }

    #[test]
    fn the_answer_follows_the_last_comment_of_lira() {
        let comments = [
            of("you", "A remark before the request."),
            of("you", &request("Do it.", BRANCH)),
            of("you", "A remark."),
            of("you", &question("The check failed.", None)),
            of("you", "Do it in another way."),
            of("someone", &question("Not Lira.", None)),
            of("you", "And test it."),
        ];
        let read = read(&comments, "you").unwrap();
        assert_eq!(read.request, "Do it.");
        assert_eq!(read.stopped.as_deref(), Some("The check failed."));
        assert_eq!(read.answer, ["Do it in another way.", "And test it."]);
        assert_eq!(read.others, [comments[5].clone()]);
    }

    #[test]
    fn the_answer_follows_the_comment_that_the_work_passed() {
        let comments = [
            of("you", &request("Do it.", BRANCH)),
            of("you", &question("The check failed.", None)),
            of("you", "Do it in another way."),
            of("you", &passed(None)),
            of("you", "Now make it louder."),
        ];
        let read = read(&comments, "you").unwrap();
        assert_eq!(read.request, "Do it.");
        assert_eq!(read.stopped, None);
        assert!(read.passed);
        assert_eq!(read.answer, ["Now make it louder."]);
        assert_eq!(read.earlier, ["Do it in another way."]);
    }

    /// A comment with the number `id` on GitHub. It was written `id`
    /// minutes after 10 o'clock.
    fn numbered(id: i64, author: &str, body: &str) -> Comment {
        Comment {
            id: Some(format!("comment {id}")),
            time: Some(format!("2026-11-01T10:{id:02}:00Z")),
            ..of(author, body)
        }
    }

    #[test]
    fn the_answer_follows_what_the_agent_of_the_last_comment_of_lira_read() {
        // The agent of `lira run` reads only the request. You write while
        // it works.
        let comments = [
            numbered(1, "you", &request("Do it.", BRANCH)),
            numbered(2, "you", "While it works."),
            numbered(3, "someone", "Me too."),
            numbered(
                4,
                "you",
                &question("The check failed.", Some(&Read::Request)),
            ),
            numbered(5, "you", "After it stopped."),
        ];
        let read = read(&comments, "you").unwrap();
        assert_eq!(read.stopped.as_deref(), Some("The check failed."));
        assert_eq!(read.answer, ["While it works.", "After it stopped."]);
        assert_eq!(read.others, [comments[2].clone()]);
        assert!(read.earlier.is_empty());
        // The agent of `lira continue` read each comment until the answer.
        let until = Read::Until {
            ids: vec!["comment 5".to_string()],
            time: Some("2026-11-01T10:05:00Z".to_string()),
        };
        let mut more = comments.to_vec();
        more.extend([
            numbered(6, "you", "While the next one works."),
            numbered(
                7,
                "you",
                &question("The Reviewer found something.", Some(&until)),
            ),
        ]);
        let read = super::read(&more, "you").unwrap();
        assert_eq!(
            read.stopped.as_deref(),
            Some("The Reviewer found something.")
        );
        assert_eq!(read.answer, ["While the next one works."]);
        assert!(read.others.is_empty());
        assert_eq!(read.earlier, ["While it works.", "After it stopped."]);
        // And so does the comment that the work passed.
        more[6] = numbered(7, "you", &passed(Some(&until)));
        let read = super::read(&more, "you").unwrap();
        assert!(read.passed);
        assert_eq!(read.answer, ["While the next one works."]);
        // When the comment that it names is gone, the answer follows the
        // last comment that is not newer than it.
        more.remove(4);
        more.push(numbered(8, "you", "Later."));
        let read = super::read(&more, "you").unwrap();
        assert_eq!(read.answer, ["While the next one works.", "Later."]);
        assert_eq!(read.earlier, ["While it works."]);
        // With no time, it follows the last comment of Lira.
        let line = more[5].body.replace(" at 2026-11-01T10:05:00Z", "");
        more[5].body = line;
        let read = super::read(&more, "you").unwrap();
        assert_eq!(read.answer, ["Later."]);
    }

    #[test]
    fn a_comment_of_the_newest_second_that_the_agent_read_and_that_lira_does_not_name_is_new() {
        let at = |id: &str, time: &str, body: &str| Comment {
            id: Some(id.to_string()),
            time: Some(time.to_string()),
            ..of("you", body)
        };
        let second = "2026-11-01T10:05:00Z";
        let until = Read::Until {
            ids: vec!["comment 2".to_string(), "review 1".to_string()],
            time: Some(second.to_string()),
        };
        // The comment of the conversation is listed before the review of
        // the same second, and was written after it.
        let comments = [
            at(
                "comment 1",
                "2026-11-01T10:00:00Z",
                &request("Do it.", BRANCH),
            ),
            at("comment 2", second, "Before the agent started."),
            at("comment 3", second, "While the agent worked."),
            at("review 1", second, "In a review."),
            at("comment 4", "2026-11-01T10:30:00Z", &passed(Some(&until))),
        ];
        let read = read(&comments, "you").unwrap();
        assert_eq!(read.answer, ["While the agent worked."]);
        assert_eq!(read.earlier, ["Before the agent started.", "In a review."]);
    }

    #[test]
    fn with_only_the_request_nothing_says_what_stopped_the_work() {
        let comments = [of("you", &request("Do it.", BRANCH)), of("you", "Go on.")];
        let read = read(&comments, "you").unwrap();
        assert_eq!(read.stopped, None);
        assert_eq!(read.answer, ["Go on."]);
    }

    #[test]
    fn with_no_request_of_the_login_there_is_nothing_to_read() {
        assert_eq!(read(&[], "you"), None);
        let comments = [
            of("someone", &request("Do it.", BRANCH)),
            of("you", "Go on."),
        ];
        assert_eq!(read(&comments, "you"), None);
        // A quote of the marker is no comment of Lira.
        let quoted = format!("> {}", request("Do it.", BRANCH));
        assert_eq!(read(&[of("you", &quoted)], "you"), None);
    }
}
