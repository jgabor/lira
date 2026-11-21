//! The spec quotes: which promises in SPEC.md a passing test quotes.
//!
//! A test quotes a promise in a comment directly above it:
//!
//! ```text
//! // spec(<heading>): <sentence>
//! #[test]
//! fn the_name() {}
//! ```

use crate::spec::{self, Promise};
use crate::testrun::Results;
use crate::{Problem, workspace};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

pub struct Score {
    /// The number of promises that a passing test quotes.
    pub proven: usize,
    /// What is wrong with the quotes. Any finding fails the check.
    pub findings: Vec<String>,
}

pub fn score(root: &Path, results: &Results) -> Result<Score, Problem> {
    let text = fs::read_to_string(root.join("SPEC.md"))
        .map_err(|e| Problem(format!("couldn't read SPEC.md: {e}")))?;
    let promises = spec::promises(&text).map_err(|e| Problem(format!("SPEC.md: {e}")))?;
    let promises: HashSet<Promise> = promises.into_iter().collect();

    let mut proven = HashSet::new();
    let mut findings = Vec::new();
    for package in workspace::packages(root)? {
        // The tests of xtask are about the check, not about Lira.
        if package.name == "xtask" {
            continue;
        }
        let mut definitions: HashMap<String, usize> = HashMap::new();
        let mut tests = Vec::new();
        for file in rust_files(&package.directory)? {
            let source = fs::read_to_string(&file)
                .map_err(|e| Problem(format!("couldn't read {}: {e}", file.display())))?;
            let shown = file
                .strip_prefix(root)
                .unwrap_or(&file)
                .display()
                .to_string();
            for name in test_functions(&source) {
                *definitions.entry(name).or_default() += 1;
            }
            for item in scan(&source) {
                match item {
                    Scanned::Orphan { line } => findings.push(format!(
                        "{shown}:{line}: a spec comment must be `// spec(<heading>): <sentence>`, directly above a #[test] function"
                    )),
                    Scanned::Test {
                        line,
                        quotes,
                        function,
                    } => tests.push((shown.clone(), line, quotes, function)),
                }
            }
        }
        for (shown, line, quotes, function) in tests {
            if definitions.get(&function).copied().unwrap_or(0) > 1 {
                findings.push(format!(
                    "{shown}:{line}: `{function}` names more than one test in package {}. The name of a test that quotes a promise must be unique in its package",
                    package.name
                ));
                continue;
            }
            let passed = results.passed(&package.name, &function);
            for quote in quotes {
                let promise = Promise {
                    heading: quote.heading,
                    text: quote.text,
                };
                if !promises.contains(&promise) {
                    findings.push(format!(
                        "{shown}:{}: SPEC.md has no promise under `{}` with this text: {}. Match the quote to SPEC.md, or remove it",
                        quote.line, promise.heading, promise.text
                    ));
                } else if passed {
                    proven.insert(promise);
                }
            }
        }
    }
    Ok(Score {
        proven: proven.len(),
        findings,
    })
}

fn rust_files(directory: &Path) -> Result<Vec<PathBuf>, Problem> {
    let mut files = Vec::new();
    let mut pending = vec![directory.to_path_buf()];
    while let Some(current) = pending.pop() {
        let entries = fs::read_dir(&current)
            .map_err(|e| Problem(format!("couldn't read {}: {e}", current.display())))?;
        for entry in entries {
            let entry = entry.map_err(|e| Problem(format!("couldn't read a folder: {e}")))?;
            let path = entry.path();
            let kind = entry
                .file_type()
                .map_err(|e| Problem(format!("couldn't read {}: {e}", path.display())))?;
            if kind.is_dir() {
                if entry.file_name() != "target" {
                    pending.push(path);
                }
            } else if path.extension().is_some_and(|e| e == "rs") {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

#[derive(Debug, PartialEq, Eq)]
struct Quote {
    line: usize,
    heading: String,
    text: String,
}

#[derive(Debug, PartialEq, Eq)]
enum Scanned {
    /// A run of spec comments, and the test that it sits above.
    Test {
        line: usize,
        quotes: Vec<Quote>,
        function: String,
    },
    /// A spec comment that is malformed, or that sits above no test.
    Orphan { line: usize },
}

/// Finds runs of `// spec(...)` comments and the test each run sits above.
fn scan(source: &str) -> Vec<Scanned> {
    let lines: Vec<&str> = source.lines().map(str::trim).collect();
    let mut found = Vec::new();
    let mut at = 0;
    while at < lines.len() {
        if !lines[at].starts_with("// spec(") {
            at += 1;
            continue;
        }
        let first_line = at + 1;
        let mut quotes = Vec::new();
        let mut malformed = false;
        while at < lines.len() && lines[at].starts_with("// spec(") {
            match parse_quote(lines[at]) {
                Some((heading, text)) => quotes.push(Quote {
                    line: at + 1,
                    heading: heading.to_string(),
                    text: text.to_string(),
                }),
                None => malformed = true,
            }
            at += 1;
        }
        let (after, is_test) = skip_attributes(&lines, at);
        at = after;
        let function = lines.get(at).copied().and_then(function_name);
        match function {
            Some(function) if is_test && !malformed => found.push(Scanned::Test {
                line: first_line,
                quotes,
                function: function.to_string(),
            }),
            _ => found.push(Scanned::Orphan { line: first_line }),
        }
    }
    found
}

/// Moves past any attributes, including ones spread over several lines, and
/// says whether one of them was `#[test]`.
fn skip_attributes(lines: &[&str], mut at: usize) -> (usize, bool) {
    let mut is_test = false;
    while at < lines.len() && lines[at].starts_with("#[") {
        is_test |= lines[at] == "#[test]";
        let mut depth = 0i32;
        while at < lines.len() {
            depth += lines[at].matches('[').count() as i32;
            depth -= lines[at].matches(']').count() as i32;
            at += 1;
            if depth <= 0 {
                break;
            }
        }
    }
    (at, is_test)
}

/// The name of every `#[test]` function, including disabled ones.
fn test_functions(source: &str) -> Vec<String> {
    let lines: Vec<&str> = source.lines().map(str::trim).collect();
    let mut names = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if *line == "#[test]" {
            let (after, _) = skip_attributes(&lines, index);
            if let Some(name) = lines.get(after).copied().and_then(function_name) {
                names.push(name.to_string());
            }
        }
    }
    names
}

fn parse_quote(line: &str) -> Option<(&str, &str)> {
    let rest = line.strip_prefix("// spec(")?;
    let (heading, text) = rest.split_once("): ")?;
    (!heading.is_empty() && !text.trim().is_empty()).then(|| (heading, text.trim()))
}

fn function_name(line: &str) -> Option<&str> {
    let mut rest = line;
    for prefix in ["pub ", "async "] {
        rest = rest.strip_prefix(prefix).unwrap_or(rest);
    }
    let rest = rest.strip_prefix("fn ")?;
    let end = rest.find(['(', '<'])?;
    Some(rest[..end].trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quote(line: usize, heading: &str, text: &str) -> Quote {
        Quote {
            line,
            heading: heading.to_string(),
            text: text.to_string(),
        }
    }

    fn test(line: usize, function: &str, quotes: Vec<Quote>) -> Scanned {
        Scanned::Test {
            line,
            quotes,
            function: function.to_string(),
        }
    }

    #[test]
    fn a_quote_names_its_heading_and_its_sentence() {
        let source = "// spec(lira --version): It prints (all of) it.\n#[test]\nfn one() {}\n";
        let quotes = vec![quote(1, "lira --version", "It prints (all of) it.")];
        assert_eq!(scan(source), [test(1, "one", quotes)]);
    }

    #[test]
    fn one_test_can_quote_several_promises() {
        let source = "\n// spec(A): One.\n// spec(B): Two.\n#[test]\n#[ignore]\nfn both() {}\n";
        let quotes = vec![quote(2, "A", "One."), quote(3, "B", "Two.")];
        assert_eq!(scan(source), [test(2, "both", quotes)]);
    }

    #[test]
    fn a_quote_above_no_test_is_an_orphan() {
        let orphan = [Scanned::Orphan { line: 1 }];
        assert_eq!(scan("// spec(A): One.\nfn one() {}\n"), orphan);
        assert_eq!(scan("// spec(A): One.\n\n#[test]\nfn one() {}\n"), orphan);
    }

    #[test]
    fn a_malformed_quote_is_an_orphan() {
        let orphan = [Scanned::Orphan { line: 1 }];
        assert_eq!(scan("// spec(A) One.\n#[test]\nfn one() {}\n"), orphan);
    }

    #[test]
    fn test_functions_include_disabled_ones() {
        let source = "#[test]\n#[cfg(windows)]\nfn a() {}\n\n#[test]\nfn b() {}\n\nfn c() {}\n";
        assert_eq!(test_functions(source), ["a", "b"]);
    }
}
