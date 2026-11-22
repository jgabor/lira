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

/// One comment on a pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    /// The login that wrote it.
    pub author: String,
    pub body: String,
}

/// The comment that keeps the request on the pull request.
pub fn request(request: &str) -> String {
    format!("{REQUEST}\nThe request of the user:\n\n{}", quote(request))
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

/// The comment that says what stops the work.
pub fn question(question: &str) -> String {
    format!("{QUESTION}\n{question}")
}

/// What `lira continue` reads from the comments of a pull request.
#[derive(Debug, PartialEq, Eq)]
pub struct Read {
    /// The request, from the first comment that has it.
    pub request: String,
    /// What the last comment of Lira says, when it is not the request.
    pub stopped: Option<String>,
    /// Your answer: each comment of `login` after the last comment of
    /// Lira, that Lira did not write.
    pub answer: Vec<String>,
    /// Each comment of someone else after the last comment of Lira.
    pub others: Vec<Comment>,
}

/// Reads the request, what stopped the work, and your answer from the
/// comments of a pull request, the oldest first. `login` is the login of
/// `gh`. Only a comment of that login can be a comment of Lira. It is
/// `None` when no comment has the request.
pub fn read(comments: &[Comment], login: &str) -> Option<Read> {
    let of_lira = |comment: &Comment| comment.author == login && marker(&comment.body).is_some();
    let request = comments
        .iter()
        .filter(|comment| comment.author == login)
        .find_map(|comment| request_in(&comment.body))?;
    let last = comments.iter().rposition(of_lira)?;
    let stopped = match marker(&comments[last].body) {
        Some((QUESTION, rest)) => Some(rest.trim().to_string()),
        _ => None,
    };
    let after = &comments[last + 1..];
    let answer = after
        .iter()
        .filter(|comment| comment.author == login)
        .map(|comment| comment.body.trim().to_string())
        .collect();
    let others = after
        .iter()
        .filter(|comment| comment.author != login)
        .cloned()
        .collect();
    Some(Read {
        request,
        stopped,
        answer,
        others,
    })
}

/// The marker on the first line of a comment of Lira, and the text after
/// that line.
fn marker(body: &str) -> Option<(&'static str, &str)> {
    let (first, rest) = body.split_once('\n').unwrap_or((body, ""));
    [REQUEST, QUESTION]
        .into_iter()
        .find(|marker| first.trim_end() == *marker)
        .map(|marker| (marker, rest))
}

/// The request in a comment that has it: the lines of its quote.
fn request_in(body: &str) -> Option<String> {
    let (REQUEST, rest) = marker(body)? else {
        return None;
    };
    let lines: Vec<&str> = rest
        .lines()
        .skip_while(|line| !line.starts_with('>'))
        .take_while(|line| line.starts_with('>'))
        .map(|line| line.strip_prefix("> ").unwrap_or(&line[1..]))
        .collect();
    let request = lines.join("\n");
    (!request.trim().is_empty()).then_some(request)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn of(author: &str, body: &str) -> Comment {
        Comment {
            author: author.to_string(),
            body: body.to_string(),
        }
    }

    #[test]
    fn the_request_comes_back_from_its_comment_as_it_was() {
        for text in [
            "Add a file.",
            "Add a file.\n\n  Make it `short`.\n> Quoted.",
        ] {
            let comments = [of("you", &request(text))];
            assert_eq!(read(&comments, "you").unwrap().request, text);
        }
        // GitHub can give the lines of a comment with a carriage return.
        let body = request("One.\n\nTwo.").replace('\n', "\r\n");
        let read = read(&[of("you", &body)], "you").unwrap();
        assert_eq!(read.request, "One.\n\nTwo.");
    }

    #[test]
    fn the_answer_follows_the_last_comment_of_lira() {
        let comments = [
            of("you", "A remark before the request."),
            of("you", &request("Do it.")),
            of("you", "A remark."),
            of("you", &question("The check failed.")),
            of("you", "Do it in another way."),
            of("someone", &question("Not Lira.")),
            of("you", "And test it."),
        ];
        let read = read(&comments, "you").unwrap();
        assert_eq!(read.request, "Do it.");
        assert_eq!(read.stopped.as_deref(), Some("The check failed."));
        assert_eq!(read.answer, ["Do it in another way.", "And test it."]);
        assert_eq!(read.others, [comments[5].clone()]);
    }

    #[test]
    fn with_only_the_request_nothing_says_what_stopped_the_work() {
        let comments = [of("you", &request("Do it.")), of("you", "Go on.")];
        let read = read(&comments, "you").unwrap();
        assert_eq!(read.stopped, None);
        assert_eq!(read.answer, ["Go on."]);
    }

    #[test]
    fn with_no_request_of_the_login_there_is_nothing_to_read() {
        assert_eq!(read(&[], "you"), None);
        let comments = [of("someone", &request("Do it.")), of("you", "Go on.")];
        assert_eq!(read(&comments, "you"), None);
        // A quote of the marker is no comment of Lira.
        let quoted = format!("> {}", request("Do it."));
        assert_eq!(read(&[of("you", &quoted)], "you"), None);
    }
}
