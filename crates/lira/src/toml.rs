//! The forms of TOML that the Lira file can have: a key with a text, a
//! number, `true` or `false`, or a list; a table; and a list of tables. Any
//! other form of TOML is an error.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Text(String),
    Number(i64),
    Bool(bool),
    List(Vec<Value>),
    /// From a `[name]` header, or from a key with dots.
    Table(Table),
    /// From `[[name]]` headers, one table for each.
    Tables(Vec<Table>),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Table {
    pub entries: Vec<Entry>,
    /// The line of the header or the key that made the table. The root has 0.
    pub line: usize,
}

impl Table {
    fn at(line: usize) -> Table {
        Table {
            entries: Vec::new(),
            line,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub key: String,
    pub value: Value,
    /// The line of the key, or of the header for a table. The first is 1.
    pub line: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Error {
    pub line: usize,
    pub message: String,
}

/// The largest count of keys with dots between them, such as `a.b.c.d`.
const MOST_KEYS: usize = 4;

/// Reads a document into its root table.
pub fn read(text: &str) -> Result<Table, Error> {
    let mut reader = Reader {
        chars: text.chars().collect(),
        at: 0,
        line: 1,
    };
    reader.refuse_control_characters()?;
    let mut root = Table::default();
    let mut open = Open::Root;
    loop {
        reader.skip_blank_lines();
        let Some(next) = reader.peek() else {
            return Ok(root);
        };
        let line = reader.line;
        if next == '[' {
            open = reader.header(&mut root)?;
        } else {
            let path = reader.key_path()?;
            reader.skip_spaces();
            if reader.peek() != Some('=') {
                return Err(reader.error("a key needs `=` and a value after it"));
            }
            reader.at += 1;
            reader.skip_spaces();
            let value = reader.value()?;
            let table = open.table(&mut root);
            insert(table, &path, value, line)?;
        }
        reader.end_of_line()?;
    }
}

/// The table that a key goes into: the root, or the one of the last header.
enum Open {
    Root,
    Table(String),
    Tables(String),
}

impl Open {
    fn table<'a>(&self, root: &'a mut Table) -> &'a mut Table {
        let name = match self {
            Open::Root => return root,
            Open::Table(name) | Open::Tables(name) => name,
        };
        let entry = root.entries.iter_mut().find(|entry| entry.key == *name);
        match entry.map(|entry| &mut entry.value) {
            Some(Value::Table(table)) => table,
            Some(Value::Tables(tables)) => tables.last_mut().expect("a header adds a table"),
            _ => unreachable!("a header makes its table before a key can use it"),
        }
    }
}

/// Puts `value` at the dotted `path` in `table`, and makes the tables on the
/// way.
fn insert(table: &mut Table, path: &[String], value: Value, line: usize) -> Result<(), Error> {
    let (last, parents) = path.split_last().expect("a key path has a key");
    let mut table = table;
    for key in parents {
        let index = match table.entries.iter().position(|entry| entry.key == *key) {
            Some(index) => index,
            None => {
                table.entries.push(Entry {
                    key: key.clone(),
                    value: Value::Table(Table::at(line)),
                    line,
                });
                table.entries.len() - 1
            }
        };
        table = match &mut table.entries[index].value {
            Value::Table(inner) => inner,
            _ => {
                let message = format!("`{key}` has a value, so it can't have keys of its own");
                return Err(Error { line, message });
            }
        };
    }
    if table.entries.iter().any(|entry| entry.key == *last) {
        let message = format!("`{}` is there two times. Remove one", path.join("."));
        return Err(Error { line, message });
    }
    table.entries.push(Entry {
        key: last.clone(),
        value,
        line,
    });
    Ok(())
}

struct Reader {
    chars: Vec<char>,
    at: usize,
    line: usize,
}

impl Reader {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.at).copied()
    }

    fn starts_with(&self, text: &str) -> bool {
        let mut rest = self.chars[self.at..].iter();
        text.chars().all(|c| rest.next() == Some(&c))
    }

    fn error(&self, message: &str) -> Error {
        Error {
            line: self.line,
            message: message.to_string(),
        }
    }

    /// An error for TOML that is correct, and that the Lira file can't have.
    fn other_form(&self, what: &str, write: &str) -> Error {
        self.error(&format!(
            "{what} is a form of TOML that the Lira file can't have. {write}"
        ))
    }

    /// TOML has no character below U+0020, and no U+007F, but the tab and the
    /// end of a line, which is `\n` or `\r\n`.
    fn refuse_control_characters(&self) -> Result<(), Error> {
        let mut line = 1;
        for (index, c) in self.chars.iter().enumerate() {
            let end_of_line =
                *c == '\n' || (*c == '\r' && self.chars.get(index + 1) == Some(&'\n'));
            let control = matches!(*c, '\0'..='\u{1f}' | '\u{7f}');
            if control && *c != '\t' && !end_of_line {
                let message = format!(
                    "there is a control character here (U+{:04X}), which TOML can't have. Remove it",
                    *c as u32
                );
                return Err(Error { line, message });
            }
            if *c == '\n' {
                line += 1;
            }
        }
        Ok(())
    }

    fn skip_spaces(&mut self) {
        while matches!(self.peek(), Some(' ' | '\t')) {
            self.at += 1;
        }
    }

    fn skip_comment(&mut self) {
        if self.peek() == Some('#') {
            while !matches!(self.peek(), Some('\n') | None) {
                self.at += 1;
            }
        }
    }

    /// Moves past spaces, comments and ends of lines.
    fn skip_blank_lines(&mut self) {
        loop {
            self.skip_spaces();
            self.skip_comment();
            match self.peek() {
                Some('\n') => {
                    self.line += 1;
                    self.at += 1;
                }
                Some('\r') => self.at += 1,
                _ => return,
            }
        }
    }

    /// After a header or a value, only a comment can be on the line.
    fn end_of_line(&mut self) -> Result<(), Error> {
        self.skip_spaces();
        self.skip_comment();
        match self.peek() {
            Some('\n' | '\r') | None => Ok(()),
            Some(_) => {
                Err(self.error("there is text after the value. Put each key on its own line"))
            }
        }
    }

    fn key(&mut self) -> Result<String, Error> {
        if matches!(self.peek(), Some('"' | '\'')) {
            return Err(self.other_form(
                "A key in quotes",
                "Write the key with letters, digits, `_` and `-` only.",
            ));
        }
        let start = self.at;
        while self
            .peek()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            self.at += 1;
        }
        if self.at == start {
            return Err(self.error("a key is expected here: letters, digits, `_` and `-`"));
        }
        Ok(self.chars[start..self.at].iter().collect())
    }

    /// Reads a key, or keys with dots between them. The format has two keys
    /// at most, and a long row of them would make tables too deep to hold.
    fn key_path(&mut self) -> Result<Vec<String>, Error> {
        let mut path = vec![self.key()?];
        loop {
            self.skip_spaces();
            if self.peek() != Some('.') {
                return Ok(path);
            }
            if path.len() == MOST_KEYS {
                return Err(self.error(&format!(
                    "a key has {MOST_KEYS} parts at most, with a dot between them"
                )));
            }
            self.at += 1;
            self.skip_spaces();
            path.push(self.key()?);
        }
    }

    /// Reads `[name]` or `[[name]]`, and makes its table in `root`.
    fn header(&mut self, root: &mut Table) -> Result<Open, Error> {
        let line = self.line;
        let list = self.starts_with("[[");
        self.at += if list { 2 } else { 1 };
        self.skip_spaces();
        let name = self.key()?;
        self.skip_spaces();
        if self.peek() == Some('.') {
            return Err(self.other_form(
                "A header with a dot",
                "Write a key with a dot in place of it, such as `models.agent`.",
            ));
        }
        let close = if list { "]]" } else { "]" };
        if !self.starts_with(close) {
            return Err(self.error(&format!("the header needs `{close}` after `{name}`")));
        }
        self.at += close.len();
        let index = root.entries.iter().position(|entry| entry.key == name);
        let existing = index.map(|index| &mut root.entries[index].value);
        match (existing, list) {
            (Some(Value::Tables(tables)), true) => tables.push(Table::at(line)),
            (None, _) => root.entries.push(Entry {
                key: name.clone(),
                value: if list {
                    Value::Tables(vec![Table::at(line)])
                } else {
                    Value::Table(Table::at(line))
                },
                line,
            }),
            (Some(_), _) => {
                let message = format!("`{name}` is there two times. Remove one");
                return Err(Error { line, message });
            }
        }
        Ok(if list {
            Open::Tables(name)
        } else {
            Open::Table(name)
        })
    }

    fn value(&mut self) -> Result<Value, Error> {
        match self.peek() {
            Some('"') => self.text('"'),
            Some('\'') => self.text('\''),
            Some('[') => self.list(),
            Some('{') => Err(self.other_form(
                "A table in `{` and `}`",
                "Write a `[table]` header, or keys with dots.",
            )),
            Some(c) if c.is_ascii_digit() || c == '+' || c == '-' => self.number(),
            Some(_) if self.word("true") => Ok(Value::Bool(true)),
            Some(_) if self.word("false") => Ok(Value::Bool(false)),
            Some(_) => {
                Err(self
                    .error("this is not a value. A text needs quotes, such as `\"cargo test\"`"))
            }
            None => Err(self.error("a value is expected after `=`")),
        }
    }

    /// Takes `word` when it is the whole next word.
    fn word(&mut self, word: &str) -> bool {
        let end = self.at + word.len();
        let whole = self.starts_with(word)
            && !self
                .chars
                .get(end)
                .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-');
        if whole {
            self.at = end;
        }
        whole
    }

    fn text(&mut self, quote: char) -> Result<Value, Error> {
        if self.starts_with(&quote.to_string().repeat(3)) {
            return Err(self.other_form(
                "A text on more than one line",
                "Write the text on one line.",
            ));
        }
        self.at += 1;
        let mut text = String::new();
        loop {
            let Some(c) = self.peek().filter(|c| *c != '\n') else {
                return Err(self.error(&format!("the text has no `{quote}` at its end")));
            };
            self.at += 1;
            if c == quote {
                return Ok(Value::Text(text));
            }
            if c == '\\' && quote == '"' {
                text.push(self.escape()?);
            } else {
                text.push(c);
            }
        }
    }

    /// Reads what follows a `\` in a text with `"`.
    fn escape(&mut self) -> Result<char, Error> {
        let c = self.peek();
        self.at += 1;
        let digits = match c {
            Some('"') => return Ok('"'),
            Some('\\') => return Ok('\\'),
            Some('n') => return Ok('\n'),
            Some('t') => return Ok('\t'),
            Some('r') => return Ok('\r'),
            Some('b') => return Ok('\u{8}'),
            Some('f') => return Ok('\u{c}'),
            Some('u') => 4,
            Some('U') => 8,
            _ => {
                return Err(self.error(
                    "`\\` starts an escape that TOML does not have. Write `\\\\` for one `\\`, or use `'` around the text",
                ));
            }
        };
        let end = (self.at + digits).min(self.chars.len());
        let hex: String = self.chars[self.at..end].iter().collect();
        let character = (hex.len() == digits && hex.chars().all(|c| c.is_ascii_hexdigit()))
            .then(|| u32::from_str_radix(&hex, 16).ok())
            .flatten()
            .and_then(char::from_u32);
        self.at = end;
        character.ok_or_else(|| self.error("the escape with `\\u` does not name a character"))
    }

    fn list(&mut self) -> Result<Value, Error> {
        self.at += 1;
        let mut values = Vec::new();
        loop {
            self.skip_blank_lines();
            match self.peek() {
                Some(']') => {
                    self.at += 1;
                    return Ok(Value::List(values));
                }
                None => return Err(self.error("the list has no `]` at its end")),
                Some('[') => {
                    return Err(self.other_form("A list in a list", "Write one list of texts."));
                }
                Some(_) => values.push(self.value()?),
            }
            self.skip_blank_lines();
            match self.peek() {
                Some(',') => self.at += 1,
                Some(']') => {}
                _ => return Err(self.error("a list needs `,` between its values")),
            }
        }
    }

    /// Reads a whole number: a sign or none, then digits, with one `_`
    /// between two digits where the file has it, and no `0` at the start.
    fn number(&mut self) -> Result<Value, Error> {
        let start = self.at;
        while self
            .peek()
            .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '_' | '.' | ':'))
        {
            self.at += 1;
        }
        let word: String = self.chars[start..self.at].iter().collect();
        let digits = word.strip_prefix(['+', '-']).unwrap_or(&word);
        let groups = |group: &str| !group.is_empty() && group.bytes().all(|b| b.is_ascii_digit());
        let whole = digits.split('_').all(groups) && (digits == "0" || !digits.starts_with('0'));
        let number = whole.then(|| word.replace('_', "").parse().ok()).flatten();
        number.map(Value::Number).ok_or_else(|| {
            self.other_form(
                &format!("`{word}` is not a whole number in decimal. Each other number, a date and a time"),
                "Write a whole number such as `12`, or a text in quotes.",
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(value: &str) -> Value {
        Value::Text(value.to_string())
    }

    /// The entries of a table as `(key, value)`.
    fn entries(table: &Table) -> Vec<(&str, &Value)> {
        let entries = table.entries.iter();
        entries
            .map(|entry| (entry.key.as_str(), &entry.value))
            .collect()
    }

    fn error(document: &str) -> (usize, String) {
        let error = read(document).unwrap_err();
        (error.line, error.message)
    }

    #[test]
    fn reads_each_kind_of_value() {
        let root = read("a = \"x\"\nb = 12\nc = true\nd = false\ne = ['y', \"z\"]\n").unwrap();
        assert_eq!(
            entries(&root),
            [
                ("a", &text("x")),
                ("b", &Value::Number(12)),
                ("c", &Value::Bool(true)),
                ("d", &Value::Bool(false)),
                ("e", &Value::List(vec![text("y"), text("z")])),
            ]
        );
    }

    #[test]
    fn an_entry_knows_its_line() {
        let root = read("# first\n\na = 1\n  b = 2 # two\n").unwrap();
        let lines: Vec<usize> = root.entries.iter().map(|entry| entry.line).collect();
        assert_eq!(lines, [3, 4]);
    }

    #[test]
    fn a_list_can_have_many_lines_with_comments() {
        let root = read("a = [\n  \"x\", # one\n  \"y\",\n]\nb = 1\n").unwrap();
        assert_eq!(
            root.entries[0].value,
            Value::List(vec![text("x"), text("y")])
        );
        assert_eq!(root.entries[1].line, 5);
    }

    #[test]
    fn a_text_has_the_escapes_of_toml() {
        let root = read(r#"a = "q\" b\\ n\n t\t u\u00e9""#).unwrap();
        assert_eq!(root.entries[0].value, text("q\" b\\ n\n t\t u\u{e9}"));
        let root = read(r"a = 'C:\dir # not a comment'").unwrap();
        assert_eq!(root.entries[0].value, text(r"C:\dir # not a comment"));
    }

    #[test]
    fn a_number_can_have_a_sign_and_underscores() {
        let root = read("a = -1_000\nb = +7\n").unwrap();
        assert_eq!(root.entries[0].value, Value::Number(-1000));
        assert_eq!(root.entries[1].value, Value::Number(7));
    }

    #[test]
    fn a_header_and_a_key_with_a_dot_make_the_same_table() {
        for document in ["[models]\nagent = \"x\"\n", "models.agent = \"x\"\n"] {
            let root = read(document).unwrap();
            let Value::Table(models) = &root.entries[0].value else {
                panic!("models is a table");
            };
            assert_eq!(root.entries[0].key, "models");
            assert_eq!(entries(models), [("agent", &text("x"))]);
        }
    }

    #[test]
    fn each_double_header_adds_a_table_to_the_list() {
        let root = read("[[score]]\nname = \"a\"\n\n[[score]]\nname = \"b\"\n").unwrap();
        let Value::Tables(scores) = &root.entries[0].value else {
            panic!("score is a list of tables");
        };
        assert_eq!(scores.len(), 2);
        assert_eq!(entries(&scores[1]), [("name", &text("b"))]);
    }

    #[test]
    fn a_number_that_toml_does_not_have_is_an_error() {
        for wrong in [
            "01",
            "1_",
            "1__0",
            "+_1",
            "_1",
            "-",
            "0x10",
            "99999999999999999999",
        ] {
            assert_eq!(error(&format!("a = 1\nb = {wrong}\n")).0, 2, "{wrong}");
        }
        assert_eq!(
            read("a = 0\nb = -0\n").unwrap().entries[1].value,
            Value::Number(0)
        );
    }

    #[test]
    fn an_escape_needs_its_count_of_hex_digits() {
        for wrong in [r#""\u+041""#, r#""\U+0000041""#, r#""\u41""#, r#""\uD800""#] {
            assert_eq!(error(&format!("a = 1\nb = {wrong}\n")).0, 2, "{wrong}");
        }
    }

    #[test]
    fn a_control_character_is_an_error_that_names_the_line() {
        for wrong in [
            "b = \"x\0y\"",
            "b = 'x\u{1b}y'",
            "b = 1 # x\0y",
            "b = 1\rc = 2",
            "b = \"x\ry\"",
        ] {
            let (line, message) = error(&format!("a = 1\n{wrong}\n"));
            assert_eq!(line, 2, "{wrong:?}");
            assert!(message.contains("control character"), "{message}");
        }
        let root = read("a = 1\r\nb = \"x\ty\"\r\n").unwrap();
        assert_eq!(root.entries[1].value, text("x\ty"));
        assert_eq!(root.entries[1].line, 2);
    }

    #[test]
    fn a_key_with_too_many_dots_is_an_error_however_long() {
        assert!(read("a.b.c.d = 1\n").is_ok());
        assert_eq!(error("a = 1\nb.c.d.e.f = 1\n").0, 2);
        let long = format!("{}last = 1\n", "a.".repeat(100_000));
        assert_eq!(error(&long).0, 1);
    }

    #[test]
    fn a_character_above_the_ascii_controls_is_text() {
        let root = read("a = \"x\u{85}y \u{9f} é\" # \u{80}\n").unwrap();
        assert_eq!(root.entries[0].value, text("x\u{85}y \u{9f} é"));
    }

    #[test]
    fn a_list_in_a_list_is_an_error_however_deep() {
        let (line, message) = error("a = 1\nb = [[\"x\"]]\n");
        assert_eq!(line, 2);
        assert!(message.contains("form of TOML"), "{message}");
        let deep = format!("a = {}{}\n", "[".repeat(100_000), "]".repeat(100_000));
        assert_eq!(error(&deep).0, 1);
    }

    #[test]
    fn a_table_knows_the_line_of_its_header() {
        let root = read("a = 1\n[[s]]\nb = 1\n\n[[s]]\n[t]\n").unwrap();
        let Value::Tables(tables) = &root.entries[1].value else {
            panic!("s is a list of tables");
        };
        assert_eq!([tables[0].line, tables[1].line], [2, 5]);
        let Value::Table(table) = &root.entries[2].value else {
            panic!("t is a table");
        };
        assert_eq!(table.line, 6);
    }

    #[test]
    fn a_key_that_is_there_two_times_is_an_error() {
        assert_eq!(error("a = 1\na = 2\n").0, 2);
        assert_eq!(error("[a]\nb = 1\n[a]\n").0, 3);
        assert_eq!(error("a.b = 1\n[a]\n").0, 2);
        assert_eq!(error("a = 1\na.b = 2\n").0, 2);
    }

    #[test]
    fn other_forms_of_toml_are_errors_that_name_the_line() {
        for document in [
            "a = 1\nb = { c = 1 }\n",
            "a = 1\nb = \"\"\"\ntext\n\"\"\"\n",
            "a = 1\nb = 1.5\n",
            "a = 1\nb = 2026-11-21\n",
            "a = 1\n\"b\" = 1\n",
            "a = 1\n[b.c]\n",
        ] {
            let (line, message) = error(document);
            assert_eq!(line, 2, "{document}");
            assert!(message.contains("form of TOML"), "{message}");
        }
    }

    #[test]
    fn text_that_is_not_toml_is_an_error_that_names_the_line() {
        for document in [
            "a = 1\nb\n",
            "a = 1\nb = cargo test\n",
            "a = 1\nb = \"no end\n",
            "a = 1\nb = [1 2]\n",
            "a = 1\nb = 1 c = 2\n",
            "a = 1\n[b\n",
            "a = 1\nb = \"\\q\"\n",
            "a = 1\nb = truest\n",
        ] {
            assert_eq!(error(document).0, 2, "{document}");
        }
    }
}
