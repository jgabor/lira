//! Scores: the lines that a check writes, and how a value compares with the
//! value on the main branch. SPEC.md, "Scores".

use crate::lira_file::Better;
use std::cmp::Ordering;
use std::fmt;

/// The number of a score: a decimal number, kept as it was written. Lira
/// compares the digits, so that no digit of a large number is lost.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Number {
    text: String,
    negative: bool,
    /// The digits before the point, with no `0` at the start.
    whole: String,
    /// The digits after the point, with no `0` at the end.
    part: String,
}

impl Number {
    /// Reads a decimal number such as `12`, `-3` or `0.25`.
    fn read(text: &str) -> Option<Number> {
        let digits = text.strip_prefix(['+', '-']).unwrap_or(text);
        let (whole, part) = digits.split_once('.').unwrap_or((digits, "0"));
        let all_digits =
            |digits: &str| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit());
        if !all_digits(whole) || !all_digits(part) {
            return None;
        }
        let whole = whole.trim_start_matches('0');
        let part = part.trim_end_matches('0');
        Some(Number {
            text: text.to_string(),
            negative: text.starts_with('-') && !(whole.is_empty() && part.is_empty()),
            whole: whole.to_string(),
            part: part.to_string(),
        })
    }

    fn compare(&self, other: &Number) -> Ordering {
        let size = |number: &Number| {
            (
                number.whole.len(),
                number.whole.clone(),
                number.part.clone(),
            )
        };
        match (self.negative, other.negative) {
            (false, true) => Ordering::Greater,
            (true, false) => Ordering::Less,
            (false, false) => size(self).cmp(&size(other)),
            (true, true) => size(other).cmp(&size(self)),
        }
    }

    /// True when this value moved against `better` from the value on the main
    /// branch.
    pub fn worse_than(&self, main: &Number, better: Better) -> bool {
        match better {
            Better::Higher => self.compare(main) == Ordering::Less,
            Better::Lower => self.compare(main) == Ordering::Greater,
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
#[derive(Clone, Debug, Default, PartialEq, Eq)]
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

    // spec(Scores): The number of a score is a decimal number, such as `12`, `-3` or `0.25`.
    #[test]
    fn the_number_of_a_score_is_a_decimal_number() {
        for right in ["12", "-3", "0.25", "+7", "007", "1.50", "9007199254740993"] {
            assert_eq!(number(right).to_string(), right);
        }
        for wrong in [
            "1e3", "0x10", ".5", "5.", "1.2.3", "--1", "1_000", "inf", "NaN", "", "-",
        ] {
            assert_eq!(Number::read(wrong), None, "{wrong}");
        }
    }

    #[test]
    fn numbers_compare_by_their_value_with_each_digit() {
        let in_order = [
            "-10",
            "-2.5",
            "-2.25",
            "-0.5",
            "0",
            "0.25",
            "0.3",
            "2",
            "2.05",
            "2.5",
            "10",
            "9007199254740992",
            "9007199254740993",
        ];
        for (index, low) in in_order.iter().enumerate() {
            for high in &in_order[index + 1..] {
                assert_eq!(
                    number(low).compare(&number(high)),
                    Ordering::Less,
                    "{low} {high}"
                );
                assert_eq!(
                    number(high).compare(&number(low)),
                    Ordering::Greater,
                    "{high} {low}"
                );
            }
        }
        for (one, other) in [
            ("0", "-0"),
            ("0", "0.0"),
            ("14", "14.0"),
            ("14", "+014.00"),
            ("-0.50", "-0.5"),
        ] {
            assert_eq!(
                number(one).compare(&number(other)),
                Ordering::Equal,
                "{one} {other}"
            );
        }
    }

    // spec(Scores): A score got worse when its value moved against its `better` from the value on the main branch.
    #[test]
    fn a_score_is_worse_when_it_moved_against_its_better() {
        assert!(number("13").worse_than(&number("14"), Better::Higher));
        assert!(number("9007199254740992").worse_than(&number("9007199254740993"), Better::Higher));
        assert!(!number("15").worse_than(&number("14"), Better::Higher));
        assert!(number("2.5").worse_than(&number("2"), Better::Lower));
        assert!(!number("1.5").worse_than(&number("2"), Better::Lower));
        for better in [Better::Higher, Better::Lower] {
            assert!(!number("14").worse_than(&number("14.0"), better));
        }
    }
}
