//! JSON, as RFC 8259 defines it. Lira reads a value from a text, and writes
//! a value as one line.

use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Json {
    Null,
    Bool(bool),
    Number(Number),
    Text(String),
    List(Vec<Json>),
    /// The keys in the order of the text. A key can be there two times.
    Object(Vec<(String, Json)>),
}

impl Json {
    /// A text value.
    pub fn text(text: &str) -> Json {
        Json::Text(text.to_string())
    }

    /// A whole number.
    pub fn number(number: i64) -> Json {
        Json::Number(Number::from(number))
    }

    /// An object with these keys, in this order.
    pub fn object<const N: usize>(entries: [(&str, Json); N]) -> Json {
        Json::Object(
            entries
                .into_iter()
                .map(|(key, value)| (key.to_string(), value))
                .collect(),
        )
    }

    /// The value of a key of an object. When the key is there two times, it
    /// is the last one. A value that is not an object has no keys.
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(entries) => entries
                .iter()
                .rev()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value),
            _ => None,
        }
    }

    /// The text, when the value is a text.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Json::Text(text) => Some(text),
            _ => None,
        }
    }

    /// The number, when the value is a whole number that fits in an `i64`.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Json::Number(number) => number.as_i64(),
            _ => None,
        }
    }

    /// The members, when the value is a list.
    pub fn as_list(&self) -> Option<&[Json]> {
        match self {
            Json::List(values) => Some(values),
            _ => None,
        }
    }
}

/// A number, kept as it was written, so that no digit is lost.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Number(String);

impl Number {
    pub fn as_i64(&self) -> Option<i64> {
        self.0.parse().ok()
    }

    pub fn as_f64(&self) -> Option<f64> {
        self.0.parse().ok()
    }
}

impl From<i64> for Number {
    fn from(number: i64) -> Number {
        Number(number.to_string())
    }
}

/// What is wrong with a text, and the byte where the reader saw it. The
/// first byte is 0.
#[derive(Debug, PartialEq, Eq)]
pub struct Error {
    pub at: usize,
    pub message: String,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "at byte {}: {}", self.at, self.message)
    }
}

/// The deepest that lists and objects can be inside each other. A deeper
/// text is an error, so that no text can use up the stack.
///
/// The limit is on what Lira reads. A value that Lira builds in its own code
/// has no test of its depth: the writer goes as deep as the value does.
const DEEPEST: usize = 128;

/// Reads one value from the whole text.
pub fn read(text: &str) -> Result<Json, Error> {
    let mut reader = Reader {
        bytes: text.as_bytes(),
        text,
        at: 0,
    };
    reader.skip_space();
    let value = reader.value(0)?;
    reader.skip_space();
    match reader.peek() {
        None => Ok(value),
        Some(_) => Err(reader.error("there is text after the value")),
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    text: &'a str,
    at: usize,
}

impl Reader<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn error(&self, message: &str) -> Error {
        Error {
            at: self.at,
            message: message.to_string(),
        }
    }

    fn skip_space(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }

    /// Takes `word` when the text has it here.
    fn word(&mut self, word: &str, value: Json) -> Result<Json, Error> {
        if self.bytes[self.at..].starts_with(word.as_bytes()) {
            self.at += word.len();
            Ok(value)
        } else {
            Err(self.error("a value is expected here"))
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json, Error> {
        match self.peek() {
            Some(b'{') => self.object(depth + 1),
            Some(b'[') => self.list(depth + 1),
            Some(b'"') => self.text().map(Json::Text),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(b't') => self.word("true", Json::Bool(true)),
            Some(b'f') => self.word("false", Json::Bool(false)),
            Some(b'n') => self.word("null", Json::Null),
            Some(_) => Err(self.error("a value is expected here")),
            None => Err(self.error("the text ends where a value is expected")),
        }
    }

    fn too_deep(&self, depth: usize) -> Result<(), Error> {
        if depth > DEEPEST {
            let message = format!("lists and objects are more than {DEEPEST} deep");
            return Err(self.error(&message));
        }
        Ok(())
    }

    fn list(&mut self, depth: usize) -> Result<Json, Error> {
        self.too_deep(depth)?;
        self.at += 1;
        let mut values = Vec::new();
        self.skip_space();
        if self.peek() == Some(b']') {
            self.at += 1;
            return Ok(Json::List(values));
        }
        loop {
            self.skip_space();
            values.push(self.value(depth)?);
            self.skip_space();
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b']') => {
                    self.at += 1;
                    return Ok(Json::List(values));
                }
                _ => return Err(self.error("a list needs `,` or `]` after a value")),
            }
        }
    }

    fn object(&mut self, depth: usize) -> Result<Json, Error> {
        self.too_deep(depth)?;
        self.at += 1;
        let mut entries = Vec::new();
        self.skip_space();
        if self.peek() == Some(b'}') {
            self.at += 1;
            return Ok(Json::Object(entries));
        }
        loop {
            self.skip_space();
            if self.peek() != Some(b'"') {
                return Err(self.error("an object needs a key in `\"` here"));
            }
            let key = self.text()?;
            self.skip_space();
            if self.peek() != Some(b':') {
                return Err(self.error("an object needs `:` after a key"));
            }
            self.at += 1;
            self.skip_space();
            entries.push((key, self.value(depth)?));
            self.skip_space();
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b'}') => {
                    self.at += 1;
                    return Ok(Json::Object(entries));
                }
                _ => return Err(self.error("an object needs `,` or `}` after a value")),
            }
        }
    }

    /// Reads a text from its first `"` to its last.
    fn text(&mut self) -> Result<String, Error> {
        self.at += 1;
        let mut text = String::new();
        loop {
            let start = self.at;
            while !matches!(self.peek(), Some(b'"' | b'\\' | 0..=0x1f) | None) {
                self.at += 1;
            }
            // The stops are ASCII, so both ends are between characters.
            text.push_str(&self.text[start..self.at]);
            match self.peek() {
                Some(b'"') => {
                    self.at += 1;
                    return Ok(text);
                }
                Some(b'\\') => {
                    self.at += 1;
                    text.push(self.escape()?);
                }
                Some(_) => {
                    return Err(self.error(
                        "a text can't have a control character. Write it as an escape, such as `\\n`",
                    ));
                }
                None => return Err(self.error("the text has no `\"` at its end")),
            }
        }
    }

    /// Reads what follows a `\`.
    fn escape(&mut self) -> Result<char, Error> {
        let escaped = self.peek();
        self.at += 1;
        Ok(match escaped {
            Some(b'"') => '"',
            Some(b'\\') => '\\',
            Some(b'/') => '/',
            Some(b'b') => '\u{8}',
            Some(b'f') => '\u{c}',
            Some(b'n') => '\n',
            Some(b'r') => '\r',
            Some(b't') => '\t',
            Some(b'u') => return self.unicode(),
            _ => {
                self.at -= 1;
                return Err(self.error("`\\` starts an escape that JSON does not have"));
            }
        })
    }

    /// Reads the four hex digits of a `\u` escape.
    fn hex(&mut self) -> Result<u32, Error> {
        let digits = self.bytes.get(self.at..self.at + 4);
        let digits = digits.filter(|digits| digits.iter().all(u8::is_ascii_hexdigit));
        let Some(digits) = digits else {
            return Err(self.error("`\\u` needs four hex digits after it"));
        };
        self.at += 4;
        let digits = std::str::from_utf8(digits).expect("hex digits are ASCII");
        Ok(u32::from_str_radix(digits, 16).expect("four hex digits are a number"))
    }

    /// Reads a `\u` escape, or the two that make one character together.
    fn unicode(&mut self) -> Result<char, Error> {
        let start = self.at;
        let first = self.hex()?;
        let code = if (0xd800..0xdc00).contains(&first) {
            if !self.bytes[self.at..].starts_with(b"\\u") {
                self.at = start;
                return Err(self.error("this `\\u` escape is the first half of a pair, and the second half is not there"));
            }
            self.at += 2;
            let second = self.hex()?;
            if !(0xdc00..0xe000).contains(&second) {
                self.at = start;
                return Err(self.error("this `\\u` escape is the first half of a pair, and the second half is not there"));
            }
            0x10000 + ((first - 0xd800) << 10) + (second - 0xdc00)
        } else {
            first
        };
        char::from_u32(code).ok_or_else(|| {
            self.at = start;
            self.error(
                "this `\\u` escape is the second half of a pair, and the first half is not there",
            )
        })
    }

    fn digits(&mut self) -> usize {
        let start = self.at;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.at += 1;
        }
        self.at - start
    }

    fn number(&mut self) -> Result<Json, Error> {
        let start = self.at;
        if self.peek() == Some(b'-') {
            self.at += 1;
        }
        let whole = self.at;
        if self.digits() == 0 {
            return Err(self.error("a number needs a digit here"));
        }
        if self.bytes[whole] == b'0' && self.at - whole > 1 {
            self.at = whole;
            return Err(self.error("a number can't start with `0` and have more digits"));
        }
        if self.peek() == Some(b'.') {
            self.at += 1;
            if self.digits() == 0 {
                return Err(self.error("a number needs a digit after its point"));
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.at += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.at += 1;
            }
            if self.digits() == 0 {
                return Err(self.error("a number needs a digit after its `e`"));
            }
        }
        Ok(Json::Number(Number(self.text[start..self.at].to_string())))
    }
}

/// Writes the value as one line, with no space that JSON does not need.
impl fmt::Display for Json {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Json::Null => f.write_str("null"),
            Json::Bool(value) => write!(f, "{value}"),
            Json::Number(Number(number)) => f.write_str(number),
            Json::Text(text) => write_text(f, text),
            Json::List(values) => {
                f.write_str("[")?;
                for (index, value) in values.iter().enumerate() {
                    if index > 0 {
                        f.write_str(",")?;
                    }
                    write!(f, "{value}")?;
                }
                f.write_str("]")
            }
            Json::Object(entries) => {
                f.write_str("{")?;
                for (index, (key, value)) in entries.iter().enumerate() {
                    if index > 0 {
                        f.write_str(",")?;
                    }
                    write_text(f, key)?;
                    write!(f, ":{value}")?;
                }
                f.write_str("}")
            }
        }
    }
}

fn write_text(f: &mut fmt::Formatter, text: &str) -> fmt::Result {
    f.write_str("\"")?;
    for character in text.chars() {
        match character {
            '"' => f.write_str("\\\"")?,
            '\\' => f.write_str("\\\\")?,
            '\n' => f.write_str("\\n")?,
            '\r' => f.write_str("\\r")?,
            '\t' => f.write_str("\\t")?,
            '\0'..='\u{1f}' => write!(f, "\\u{:04x}", character as u32)?,
            _ => write!(f, "{character}")?,
        }
    }
    f.write_str("\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(value: &str) -> Json {
        Json::Text(value.to_string())
    }

    fn number(value: &str) -> Json {
        Json::Number(Number(value.to_string()))
    }

    #[test]
    fn reads_each_form_of_json() {
        let value = read(
            " { \"a\" : [ 1 , -2.5e+3 , true , false , null ] , \"b\" : { } , \"c\" : \"x\" } ",
        )
        .unwrap();
        let list = vec![
            number("1"),
            number("-2.5e+3"),
            Json::Bool(true),
            Json::Bool(false),
            Json::Null,
        ];
        let expected = Json::Object(vec![
            ("a".to_string(), Json::List(list)),
            ("b".to_string(), Json::Object(vec![])),
            ("c".to_string(), text("x")),
        ]);
        assert_eq!(value, expected);
    }

    #[test]
    fn reads_each_escape_of_a_text() {
        let value = read(r#""q\" b\\ s\/ \b\f\n\r\t \u00e9 \ud834\udd1e é""#).unwrap();
        assert_eq!(
            value,
            text("q\" b\\ s/ \u{8}\u{c}\n\r\t \u{e9} \u{1d11e} é")
        );
    }

    #[test]
    fn half_a_pair_of_escapes_is_an_error() {
        for wrong in [
            r#""\ud834""#,
            r#""\ud834x""#,
            r#""\ud834\u0041""#,
            r#""\udd1e""#,
            r#""\ud834\ud834""#,
        ] {
            let error = read(wrong).unwrap_err();
            assert!(error.message.contains("half of a pair"), "{wrong}: {error}");
            assert_eq!(error.at, 3, "{wrong}");
        }
    }

    #[test]
    fn a_number_keeps_each_digit() {
        let value = read("123456789012345678901234567890.123456789").unwrap();
        assert_eq!(
            value.to_string(),
            "123456789012345678901234567890.123456789"
        );
        let Json::Number(small) = read("-42").unwrap() else {
            panic!("-42 is a number");
        };
        assert_eq!(small.as_i64(), Some(-42));
        assert_eq!(small.as_f64(), Some(-42.0));
        assert_eq!(Json::Number(Number::from(7)).to_string(), "7");
        let Json::Number(part) = read("1.5").unwrap() else {
            panic!("1.5 is a number");
        };
        assert_eq!(part.as_i64(), None);
    }

    #[test]
    fn an_error_says_where() {
        for (wrong, at) in [
            ("", 0),
            ("[1,]", 3),
            ("[1 2]", 3),
            ("{\"a\":1,}", 7),
            ("{a:1}", 1),
            ("{\"a\" 1}", 5),
            ("\"a\nb\"", 2),
            ("\"a\\qb\"", 3),
            ("\"\\u12g4\"", 3),
            ("\"open", 5),
            ("01", 0),
            ("1.", 2),
            ("1e", 2),
            ("-", 1),
            ("tru", 0),
            ("nul", 0),
            ("1 2", 2),
            ("[1", 2),
            ("\u{feff}1", 0),
        ] {
            assert_eq!(read(wrong).unwrap_err().at, at, "{wrong:?}");
        }
    }

    #[test]
    fn a_text_that_is_too_deep_is_an_error_however_deep() {
        let fits = format!("{}{}", "[".repeat(DEEPEST), "]".repeat(DEEPEST));
        assert!(read(&fits).is_ok());
        let one_more = format!("[{fits}]");
        assert_eq!(read(&one_more).unwrap_err().at, DEEPEST);
        // The two largest texts of the JSONTestSuite, which are not valid.
        assert!(read(&"[".repeat(100_000)).is_err());
        assert!(read(&"[{\"\":".repeat(50_000)).is_err());
    }

    #[test]
    fn writes_one_line_that_reads_as_the_same_value() {
        let value = Json::Object(vec![
            (
                "text".to_string(),
                text("a \"b\" \\ \n\r\t \u{0} \u{1f} é \u{2028} \u{1d11e}"),
            ),
            (
                "list".to_string(),
                Json::List(vec![number("0"), Json::Null, Json::Bool(false)]),
            ),
            ("key \n".to_string(), Json::Object(vec![])),
        ]);
        let line = value.to_string();
        assert!(!line.contains('\n'), "{line}");
        assert_eq!(
            line,
            "{\"text\":\"a \\\"b\\\" \\\\ \\n\\r\\t \\u0000 \\u001f é \u{2028} \u{1d11e}\",\"list\":[0,null,false],\"key \\n\":{}}"
        );
        assert_eq!(read(&line).unwrap(), value);
    }

    #[test]
    fn builds_a_value_and_gets_its_parts() {
        let value = Json::object([
            ("name", Json::text("lira")),
            ("count", Json::number(-3)),
            ("list", Json::List(vec![Json::Null])),
        ]);
        assert_eq!(
            value.to_string(),
            "{\"name\":\"lira\",\"count\":-3,\"list\":[null]}"
        );
        assert_eq!(value.get("name").and_then(Json::as_text), Some("lira"));
        assert_eq!(value.get("count").and_then(Json::as_i64), Some(-3));
        assert_eq!(
            value.get("list").and_then(Json::as_list),
            Some(&[Json::Null][..])
        );
        assert_eq!(value.get("other"), None);

        // A value of another kind gives nothing.
        assert_eq!(Json::text("x").get("name"), None);
        assert_eq!(Json::number(1).as_text(), None);
        assert_eq!(Json::text("1").as_i64(), None);
        assert_eq!(number("1.5").as_i64(), None);
        assert_eq!(Json::Null.as_list(), None);

        // The last of two keys with the same name counts.
        let twice = read("{\"a\":1,\"a\":2}").unwrap();
        assert_eq!(twice.get("a"), Some(&Json::number(2)));
    }
}
