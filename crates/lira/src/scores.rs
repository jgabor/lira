//! Scores: the lines that a check writes, and how a value compares with the
//! value on the main branch. SPEC.md, "Scores".

use crate::lira_file::Better;
use std::fmt;

/// The number of a score, kept as it was written.
#[derive(Clone, Debug, PartialEq)]
pub struct Number {
    text: String,
    value: f64,
}

impl Number {
    /// Reads a decimal number: digits, with a sign and a point or without.
    fn read(text: &str) -> Option<Number> {
        let decimal = text
            .bytes()
            .all(|b| b.is_ascii_digit() || b"+-.".contains(&b));
        let digit = text.bytes().any(|b| b.is_ascii_digit());
        let value: f64 = text.parse().ok().filter(|_| decimal && digit)?;
        value.is_finite().then(|| Number {
            text: text.to_string(),
            value,
        })
    }

    /// True when this value moved against `better` from the value on the main
    /// branch.
    pub fn worse_than(&self, main: &Number, better: Better) -> bool {
        match better {
            Better::Higher => self.value < main.value,
            Better::Lower => self.value > main.value,
        }
    }
}

impl fmt::Display for Number {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.text)
    }
}

/// The scores of one check, or of the main branch, in the order they were
/// written.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scores {
    lines: Vec<(String, Number)>,
}

impl Scores {
    /// Reads score lines. Each is `<name> = <number>`. An empty line is not a
    /// score line. An error names its line.
    pub fn read(text: &str) -> Result<Scores, String> {
        let mut scores = Scores::default();
        for (index, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let number = index + 1;
            let wrong = || {
                format!("line {number}: a score line is `<name> = <number>`, and this is `{line}`")
            };
            let (name, value) = line.split_once('=').ok_or_else(wrong)?;
            let name = name.trim();
            let value = Number::read(value.trim()).ok_or_else(wrong)?;
            if name.is_empty() || name.contains(char::is_whitespace) {
                return Err(wrong());
            }
            if scores.get(name).is_some() {
                return Err(format!(
                    "line {number}: the score `{name}` is there two times. Write one line for each score"
                ));
            }
            scores.lines.push((name.to_string(), value));
        }
        Ok(scores)
    }

    pub fn get(&self, name: &str) -> Option<&Number> {
        let line = self.lines.iter().find(|(known, _)| known == name);
        line.map(|(_, number)| number)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.lines.iter().map(|(name, _)| name.as_str())
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn number(text: &str) -> Number {
        Number::read(text).unwrap()
    }

    #[test]
    fn reads_each_line_as_a_name_and_a_number() {
        let scores = Scores::read("spec = 14\n\n  size=2.5  \nlow = -3\n").unwrap();
        assert_eq!(scores.names().collect::<Vec<_>>(), ["spec", "size", "low"]);
        assert_eq!(scores.get("size").unwrap().to_string(), "2.5");
        assert_eq!(scores.get("none"), None);
    }

    #[test]
    fn a_line_that_is_not_a_score_line_is_an_error_that_names_the_line() {
        for wrong in [
            "spec",
            "spec = many",
            "= 1",
            "two words = 1",
            "spec = 1e3",
            "spec = inf",
            "spec = NaN",
            "spec = .",
            "spec = ",
        ] {
            let error = Scores::read(&format!("a = 1\n{wrong}\n")).unwrap_err();
            assert!(
                error.starts_with("line 2: a score line is `<name> = <number>`"),
                "{wrong}: {error}"
            );
        }
    }

    // spec(Scores): A score got worse when its value moved against its `better` from the value on the main branch.
    #[test]
    fn a_score_is_worse_when_it_moved_against_its_better() {
        assert!(number("13").worse_than(&number("14"), Better::Higher));
        assert!(!number("15").worse_than(&number("14"), Better::Higher));
        assert!(number("2.5").worse_than(&number("2"), Better::Lower));
        assert!(!number("1.5").worse_than(&number("2"), Better::Lower));
        for better in [Better::Higher, Better::Lower] {
            assert!(!number("14").worse_than(&number("14.0"), better));
        }
    }
}
