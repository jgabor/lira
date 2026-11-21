//! Takes the promises from SPEC.md.
//!
//! - Code fences are ignored. A promise belongs to the nearest heading above
//!   it, and headings are unique.
//! - A paragraph or list item is unwrapped, then split after `.`, `?` or `!`
//!   followed by a space, outside code spans. Each part is a promise.
//! - A table row that is not the header is one promise.
//! - A part that ends in a colon is not a promise.
//! - The text of a promise is its source with whitespace collapsed and any
//!   list marker removed.
//! - The same text twice under one heading counts once.
//! - A paragraph, list item or table row that starts with `*Proposed*`,
//!   `*Proposed:*` or `**Planned.**` has no promises.

use std::collections::HashSet;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Promise {
    pub heading: String,
    pub text: String,
}

const MARKERS: [&str; 3] = ["*Proposed*", "*Proposed:*", "**Planned.**"];

/// Returns the promises, in document order.
pub fn promises(spec: &str) -> Result<Vec<Promise>, String> {
    let lines: Vec<&str> = spec.lines().collect();
    let mut extractor = Extractor::default();
    let mut headings = HashSet::new();
    let mut fence: Option<(u8, usize)> = None;
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if let Some(open) = fence {
            if closes_fence(trimmed, open) {
                fence = None;
            }
            continue;
        }
        if !trimmed.is_empty() && !line.starts_with("  ") {
            extractor.marked_item = false;
        }
        if let Some(open) = opens_fence(trimmed) {
            extractor.flush();
            fence = Some(open);
        } else if trimmed.is_empty() {
            extractor.flush();
        } else if let Some(title) = heading(line) {
            extractor.flush();
            if !headings.insert(title.to_string()) {
                return Err(format!("duplicate heading `{title}`"));
            }
            extractor.heading = Some(title.to_string());
        } else if trimmed.starts_with('|') {
            extractor.flush();
            let next_is_separator = lines.get(index + 1).is_some_and(|l| is_separator(l));
            if !next_is_separator && !is_separator(line) {
                extractor.block.push(trimmed);
                extractor.flush_row();
            }
        } else {
            let body = trimmed.strip_prefix("> ").unwrap_or(trimmed);
            if let Some(item) = list_item(body) {
                extractor.flush();
                extractor.marked_item = false;
                extractor.in_item = true;
                extractor.block.push(item);
            } else {
                extractor.block.push(body);
            }
        }
    }
    extractor.flush();
    Ok(extractor.promises)
}

#[derive(Default)]
struct Extractor<'a> {
    heading: Option<String>,
    block: Vec<&'a str>,
    seen: HashSet<Promise>,
    promises: Vec<Promise>,
    /// The current block began as a list item.
    in_item: bool,
    /// A marker at the start of this list item covers what follows it.
    marked_item: bool,
}

impl Extractor<'_> {
    /// Takes the text of the block, unless a marker covers it. The marker of
    /// a table row is at the start of its first cell.
    fn take_text(&mut self, row: bool) -> Option<String> {
        let text = self.block.join(" ");
        self.block.clear();
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        let start = if row {
            text.trim_start_matches('|').trim_start()
        } else {
            text.as_str()
        };
        let marked = MARKERS.iter().any(|m| start.starts_with(m)) || self.marked_item;
        self.marked_item = marked && (self.in_item || self.marked_item);
        self.in_item = false;
        (!text.is_empty() && !marked).then_some(text)
    }

    /// Ends a paragraph or list item, which holds any number of sentences.
    fn flush(&mut self) {
        if let Some(text) = self.take_text(false) {
            for sentence in sentences(&text) {
                self.add(sentence);
            }
        }
    }

    /// Ends a table row, which is one promise however many full stops it has.
    fn flush_row(&mut self) {
        if let Some(text) = self.take_text(true) {
            self.add(text);
        }
    }

    fn add(&mut self, text: String) {
        let Some(heading) = &self.heading else {
            return;
        };
        let promise = Promise {
            heading: heading.clone(),
            text,
        };
        if !promise.text.ends_with(':') && self.seen.insert(promise.clone()) {
            self.promises.push(promise);
        }
    }
}

fn opens_fence(line: &str) -> Option<(u8, usize)> {
    let byte = *line.as_bytes().first()?;
    if byte != b'`' && byte != b'~' {
        return None;
    }
    let length = line.bytes().take_while(|b| *b == byte).count();
    (length >= 3).then_some((byte, length))
}

fn closes_fence(line: &str, (byte, length): (u8, usize)) -> bool {
    let run = line.bytes().take_while(|b| *b == byte).count();
    run >= length && line[run..].trim().is_empty()
}

fn heading(line: &str) -> Option<&str> {
    let hashes = line.bytes().take_while(|b| *b == b'#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    line[hashes..]
        .strip_prefix(' ')
        .map(str::trim)
        .filter(|title| !title.is_empty())
}

fn is_separator(line: &str) -> bool {
    let line = line.trim();
    line.starts_with('|')
        && line.contains('-')
        && line.bytes().all(|b| matches!(b, b'|' | b'-' | b':' | b' '))
}

fn list_item(line: &str) -> Option<&str> {
    for marker in ["- ", "* ", "+ "] {
        if let Some(item) = line.strip_prefix(marker) {
            return Some(item);
        }
    }
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    (digits > 0).then(|| line[digits..].strip_prefix(". "))?
}

fn backticks(bytes: &[u8]) -> usize {
    bytes.iter().take_while(|b| **b == b'`').count()
}

/// Finds the end of the code span that a run of `length` backticks opens. A
/// run with no matching close opens no span, and is literal text.
fn closing_run(bytes: &[u8], from: usize, length: usize) -> Option<usize> {
    let mut at = from;
    while at < bytes.len() {
        if bytes[at] == b'`' {
            let run = backticks(&bytes[at..]);
            if run == length {
                return Some(at + run);
            }
            at += run;
        } else {
            at += 1;
        }
    }
    None
}

/// Splits after `.`, `?`, or `!` followed by a space, outside code spans.
fn sentences(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut start = 0;
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'\\' {
            at += 2;
            continue;
        }
        if bytes[at] == b'`' {
            let run = backticks(&bytes[at..]);
            at = closing_run(bytes, at + run, run).unwrap_or(at + run);
            continue;
        }
        if matches!(bytes[at], b'.' | b'?' | b'!') && bytes.get(at + 1) == Some(&b' ') {
            let sentence = &text[start..=at];
            if !sentence.ends_with("e.g.") && !sentence.ends_with("i.e.") {
                out.push(sentence.to_string());
                start = at + 2;
            }
        }
        at += 1;
    }
    if start < text.len() {
        out.push(text[start..].to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(spec: &str) -> Vec<String> {
        let promises = promises(spec).unwrap();
        promises.into_iter().map(|p| p.text).collect()
    }

    #[test]
    fn a_wrapped_paragraph_is_unwrapped_and_split() {
        let spec = "# A\n\nThe first sentence\nwraps here. The second one? Yes!\n";
        assert_eq!(
            texts(spec),
            ["The first sentence wraps here.", "The second one?", "Yes!"]
        );
    }

    #[test]
    fn a_promise_belongs_to_the_nearest_heading() {
        let spec = "# A\n\nOne.\n\n## B\n\nTwo.\n";
        let promises = promises(spec).unwrap();
        assert_eq!(promises[0].heading, "A");
        assert_eq!(promises[1].heading, "B");
    }

    #[test]
    fn text_before_any_heading_is_not_a_promise() {
        assert_eq!(texts("Stray.\n\n# A\n\nOne.\n"), ["One."]);
    }

    #[test]
    fn duplicate_headings_are_an_error() {
        let error = promises("# A\n\n## B\n\n## B\n").unwrap_err();
        assert!(error.contains("B"), "{error}");
    }

    #[test]
    fn code_fences_are_ignored() {
        let spec = "# A\n\n```\nNot a promise.\n# Not a heading\n```\n\nOne.\n";
        assert_eq!(texts(spec), ["One."]);
    }

    #[test]
    fn a_longer_fence_can_hold_a_shorter_one() {
        let spec = "# A\n\n`````\n````\nNot a promise.\n````\n`````\n\nOne.\n";
        assert_eq!(texts(spec), ["One."]);
    }

    #[test]
    fn a_fence_only_closes_with_at_least_its_own_length() {
        let spec = "# A\n\n````\n```\nNot a promise.\n````\n\nOne.\n";
        assert_eq!(texts(spec), ["One."]);
    }

    #[test]
    fn a_full_stop_inside_a_code_span_does_not_split() {
        let spec = "# A\n\nUse `a. b` or ``c. `d` e.`` here. Next.\n";
        assert_eq!(texts(spec), ["Use `a. b` or ``c. `d` e.`` here.", "Next."]);
    }

    #[test]
    fn a_full_stop_needs_a_following_space() {
        assert_eq!(
            texts("# A\n\nSee lira.toml and v0.1.0 now.\n"),
            ["See lira.toml and v0.1.0 now."]
        );
    }

    #[test]
    fn abbreviations_do_not_end_a_sentence() {
        let spec = "# A\n\nUse a tool, e.g. this one, i.e. mine. Next.\n";
        assert_eq!(
            texts(spec),
            ["Use a tool, e.g. this one, i.e. mine.", "Next."]
        );
    }

    #[test]
    fn list_markers_are_removed_and_items_unwrapped() {
        let spec = "# A\n\n- First item\n  wraps.\n* Second.\n+ Third.\n1. Fourth.\n  - Nested.\n";
        assert_eq!(
            texts(spec),
            [
                "First item wraps.",
                "Second.",
                "Third.",
                "Fourth.",
                "Nested."
            ]
        );
    }

    #[test]
    fn emphasis_at_line_start_is_not_a_list_marker() {
        assert_eq!(
            texts("# A\n\n**Bold.** Rest here.\n"),
            ["**Bold.** Rest here."]
        );
    }

    #[test]
    fn a_table_row_is_one_promise_without_header_or_separator() {
        let spec = "# A\n\n| H1 | H2 |\n|---|:--:|\n| a. b. | c |\n| d  |  e |\n";
        assert_eq!(texts(spec), ["| a. b. | c |", "| d | e |"]);
    }

    #[test]
    fn a_part_ending_in_a_colon_is_not_a_promise() {
        let spec = "# A\n\nThese apply:\n\n- One.\n\nFirst. Then this:\n";
        assert_eq!(texts(spec), ["One.", "First."]);
    }

    #[test]
    fn whitespace_is_collapsed() {
        assert_eq!(texts("# A\n\nOne   two\t three.\n"), ["One two three."]);
    }

    #[test]
    fn the_same_text_under_one_heading_counts_once() {
        assert_eq!(texts("# A\n\nSame. Same.\n\nSame.\n"), ["Same."]);
    }

    #[test]
    fn the_same_text_under_two_headings_counts_twice() {
        assert_eq!(texts("# A\n\nSame.\n\n# B\n\nSame.\n").len(), 2);
    }

    #[test]
    fn a_proposed_marker_covers_its_whole_paragraph() {
        let spec = "# A\n\nKept.\n\n*Proposed:* Not this. Nor this.\n\nKept too.\n";
        assert_eq!(texts(spec), ["Kept.", "Kept too."]);
    }

    #[test]
    fn a_bare_proposed_marker_counts() {
        assert_eq!(
            texts("# A\n\n*Proposed* Not this. Nor this.\n\nKept.\n"),
            ["Kept."]
        );
    }

    #[test]
    fn a_planned_marker_covers_its_whole_paragraph() {
        let spec = "# A\n\n**Planned.** After this version. And that.\n\nKept.\n";
        assert_eq!(texts(spec), ["Kept."]);
    }

    #[test]
    fn a_marker_after_the_start_covers_nothing() {
        let spec = "# A\n\nWhat starts with *Proposed* is not decided. Yes.\n";
        assert_eq!(
            texts(spec),
            ["What starts with *Proposed* is not decided.", "Yes."]
        );
    }

    #[test]
    fn a_marker_covers_its_list_item_and_table_row_only() {
        let spec =
            "# A\n\n- *Proposed:* no.\n- Yes.\n\n| H |\n|---|\n| *Proposed:* no |\n| yes |\n";
        assert_eq!(texts(spec), ["Yes.", "| yes |"]);
    }

    #[test]
    fn a_marker_covers_every_paragraph_of_its_list_item() {
        let spec = "# A\n\n- *Proposed:* no.\n\n  Still no.\n\n- Yes.\n\nAlso yes.\n";
        assert_eq!(texts(spec), ["Yes.", "Also yes."]);
    }

    #[test]
    fn an_unmatched_backtick_opens_no_span() {
        let spec = "# A\n\nA literal ` character. Next sentence.\n";
        assert_eq!(texts(spec), ["A literal ` character.", "Next sentence."]);
    }

    #[test]
    fn an_escaped_backtick_opens_no_span() {
        let spec = "# A\n\nLiteral \\`a. b\\` is text. Next.\n";
        assert_eq!(texts(spec), ["Literal \\`a.", "b\\` is text.", "Next."]);
    }

    #[test]
    fn a_blockquote_is_a_paragraph() {
        assert_eq!(
            texts("# A\n\n> Quoted text\n> wraps.\n"),
            ["Quoted text wraps."]
        );
    }

    #[test]
    fn the_real_spec_has_promises_and_no_marked_ones() {
        let promises = promises(include_str!("../../SPEC.md")).unwrap();
        assert!(!promises.is_empty());
        assert!(promises.iter().all(|p| !p.text.contains('\n')));
        let marked = |p: &Promise| MARKERS.iter().any(|m| p.text.contains(m));
        let marked: Vec<&Promise> = promises.iter().filter(|p| marked(p)).collect();
        // The one paragraph that names the markers does not start with one.
        assert!(
            marked.iter().all(|p| p.heading == "Lira specification"),
            "{marked:#?}"
        );
    }
}
