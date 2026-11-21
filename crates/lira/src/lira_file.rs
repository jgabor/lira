//! The Lira file: `lira.toml` at the root of a repository. SPEC.md, "Format
//! of the Lira file", defines each key.

use crate::toml::{self, Entry, Table, Value};
use std::fmt;

/// The name of the Lira file.
pub const NAME: &str = "lira.toml";

/// The paths that are protected in each repository. SPEC.md, "Protected
/// paths".
const ALWAYS_PROTECTED: [&str; 2] = [NAME, ".github/"];

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LiraFile {
    /// The check command.
    pub check: String,
    /// A command that Lira runs in each new worktree.
    pub setup: Option<String>,
    /// The protected paths that the file names.
    pub protected: Vec<String>,
    /// The agent limit for this repository.
    pub agents: Option<u32>,
    pub models: Models,
    pub reviewer: Reviewer,
    /// The scores, in the order of the file.
    pub scores: Vec<Score>,
    pub planner: Planner,
}

/// The AI model for each agent.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Models {
    pub agent: Option<String>,
    pub planner: Option<String>,
    pub reviewer: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reviewer {
    /// The harnesses for the Reviewer, in order of preference.
    pub harnesses: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Score {
    pub name: String,
    pub better: Better,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Better {
    Higher,
    Lower,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Planner {
    /// If the Planner runs.
    pub on: Option<bool>,
    /// The path of the Planner's instructions.
    pub instructions: Option<String>,
}

/// What is wrong with a Lira file. It prints as `lira.toml:<line>: <what>`.
#[derive(Debug, PartialEq, Eq)]
pub struct Error {
    pub line: Option<usize>,
    pub message: String,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.line {
            Some(line) => write!(f, "{NAME}:{line}: {}", self.message),
            None => write!(f, "{NAME}: {}", self.message),
        }
    }
}

impl LiraFile {
    /// Reads the text of a Lira file.
    pub fn read(text: &str) -> Result<LiraFile, Error> {
        let root = toml::read(text).map_err(|error| Error {
            line: Some(error.line),
            message: error.message,
        })?;
        let mut file = LiraFile::default();
        let mut check = None;
        for entry in &root.entries {
            match entry.key.as_str() {
                "check" => check = Some(text_of(entry, "check")?),
                "setup" => file.setup = Some(text_of(entry, "setup")?),
                "protected" => file.protected = protected(entry)?,
                "agents" => file.agents = Some(agents(entry)?),
                "models" => file.models = models(entry)?,
                "reviewer" => file.reviewer = reviewer(entry)?,
                "score" => file.scores = scores(entry)?,
                "planner" => file.planner = planner(entry)?,
                key => {
                    return Err(unknown(
                        entry,
                        key,
                        "check, setup, protected, agents, models, reviewer, score and planner",
                    ));
                }
            }
        }
        file.check = check.ok_or_else(|| Error {
            line: None,
            message: "there is no `check`. Add the check command, such as `check = \"cargo test\"`"
                .to_string(),
        })?;
        Ok(file)
    }

    /// True when `path` is a protected path. `path` is relative to the root
    /// of the repository, as git prints it.
    pub fn protects(&self, path: &str) -> bool {
        let named = self.protected.iter().map(String::as_str);
        ALWAYS_PROTECTED.into_iter().chain(named).any(|protected| {
            match protected.strip_suffix('/') {
                Some(_) => path.starts_with(protected),
                None => path == protected,
            }
        })
    }
}

fn error(entry: &Entry, message: String) -> Error {
    Error {
        line: Some(entry.line),
        message,
    }
}

fn unknown(entry: &Entry, key: &str, known: &str) -> Error {
    error(
        entry,
        format!("unknown key `{key}`. Correct its name, or remove it. The keys here are {known}"),
    )
}

fn text_of(entry: &Entry, key: &str) -> Result<String, Error> {
    match &entry.value {
        Value::Text(text) => Ok(text.clone()),
        _ => Err(error(
            entry,
            format!("`{key}` must be a text in quotes, such as `{key} = \"...\"`"),
        )),
    }
}

fn texts_of(entry: &Entry, key: &str) -> Result<Vec<String>, Error> {
    let wrong = || {
        error(
            entry,
            format!("`{key}` must be a list of texts, such as `{key} = [\"...\"]`"),
        )
    };
    let Value::List(values) = &entry.value else {
        return Err(wrong());
    };
    let texts = values.iter().map(|value| match value {
        Value::Text(text) => Ok(text.clone()),
        _ => Err(wrong()),
    });
    texts.collect()
}

fn table_of<'a>(entry: &'a Entry, key: &str) -> Result<&'a Table, Error> {
    match &entry.value {
        Value::Table(table) => Ok(table),
        _ => Err(error(
            entry,
            format!("`{key}` must be a table: write `[{key}]` and its keys under it"),
        )),
    }
}

fn protected(entry: &Entry) -> Result<Vec<String>, Error> {
    let paths = texts_of(entry, "protected")?;
    // Two ways to write one path would let a path look protected when it is
    // not, so there is only one way.
    let from_root = |path: &&String| {
        let parts = path.strip_suffix('/').unwrap_or(path).split('/');
        !path.is_empty()
            && parts
                .into_iter()
                .all(|part| !matches!(part, "" | "." | ".."))
    };
    if let Some(path) = paths.iter().find(|path| !from_root(path)) {
        return Err(error(
            entry,
            format!(
                "`{path}` in `protected` must be a path from the root of the repository, with no `/` at its start and no `.` or `..` part, such as `SPEC.md` or `docs/`"
            ),
        ));
    }
    Ok(paths)
}

fn agents(entry: &Entry) -> Result<u32, Error> {
    let number = match &entry.value {
        Value::Number(number) => u32::try_from(*number).ok(),
        _ => None,
    };
    number.ok_or_else(|| {
        error(
            entry,
            "`agents` must be a number that is 0 or more, such as `agents = 2`".to_string(),
        )
    })
}

fn models(entry: &Entry) -> Result<Models, Error> {
    let mut models = Models::default();
    for entry in &table_of(entry, "models")?.entries {
        match entry.key.as_str() {
            "agent" => models.agent = Some(text_of(entry, "models.agent")?),
            "planner" => models.planner = Some(text_of(entry, "models.planner")?),
            "reviewer" => models.reviewer = Some(text_of(entry, "models.reviewer")?),
            key => {
                return Err(unknown(
                    entry,
                    &format!("models.{key}"),
                    "agent, planner and reviewer",
                ));
            }
        }
    }
    Ok(models)
}

fn reviewer(entry: &Entry) -> Result<Reviewer, Error> {
    let mut reviewer = Reviewer::default();
    for entry in &table_of(entry, "reviewer")?.entries {
        match entry.key.as_str() {
            "harnesses" => reviewer.harnesses = texts_of(entry, "reviewer.harnesses")?,
            key => return Err(unknown(entry, &format!("reviewer.{key}"), "harnesses")),
        }
    }
    Ok(reviewer)
}

fn planner(entry: &Entry) -> Result<Planner, Error> {
    let mut planner = Planner::default();
    for entry in &table_of(entry, "planner")?.entries {
        match entry.key.as_str() {
            "on" => match &entry.value {
                Value::Bool(on) => planner.on = Some(*on),
                _ => {
                    let message = "`planner.on` must be `true` or `false`, with no quotes";
                    return Err(error(entry, message.to_string()));
                }
            },
            "instructions" => {
                planner.instructions = Some(text_of(entry, "planner.instructions")?);
            }
            key => {
                return Err(unknown(
                    entry,
                    &format!("planner.{key}"),
                    "on and instructions",
                ));
            }
        }
    }
    Ok(planner)
}

fn scores(entry: &Entry) -> Result<Vec<Score>, Error> {
    let Value::Tables(tables) = &entry.value else {
        return Err(error(
            entry,
            "`score` must be a list of tables: write `[[score]]` for each score, with a `name` and a `better` under it".to_string(),
        ));
    };
    let mut scores: Vec<Score> = Vec::new();
    for table in tables {
        let mut name = None;
        let mut better = None;
        for entry in &table.entries {
            match entry.key.as_str() {
                "name" => name = Some((text_of(entry, "name")?, entry)),
                "better" => better = Some(better_of(entry)?),
                key => return Err(unknown(entry, &format!("score.{key}"), "name and better")),
            }
        }
        let missing = |key: &str| Error {
            line: Some(table.line),
            message: format!("this score has no `{key}`. Each score has a `name` and a `better`"),
        };
        let (name, name_entry) = name.ok_or_else(|| missing("name"))?;
        let better = better.ok_or_else(|| missing("better"))?;
        if scores.iter().any(|score| score.name == name) {
            return Err(error(
                name_entry,
                format!("two scores have the name `{name}`. Give each score its own name"),
            ));
        }
        scores.push(Score { name, better });
    }
    Ok(scores)
}

fn better_of(entry: &Entry) -> Result<Better, Error> {
    match &entry.value {
        Value::Text(text) if text == "higher" => Ok(Better::Higher),
        Value::Text(text) if text == "lower" => Ok(Better::Lower),
        _ => Err(error(
            entry,
            "`better` must be `\"higher\"` or `\"lower\"`".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(text: &str) -> LiraFile {
        LiraFile::read(text).unwrap_or_else(|error| panic!("{error}"))
    }

    /// What the error for `text` prints.
    fn error_of(text: &str) -> String {
        LiraFile::read(text).unwrap_err().to_string()
    }

    fn texts(values: &[&str]) -> Vec<String> {
        values.iter().map(ToString::to_string).collect()
    }

    const CHECK: &str = "check = \"cargo test\"\n";

    #[test]
    fn reads_the_lira_file_of_this_repository() {
        let file = read(include_str!("../../../lira.toml"));
        assert_eq!(file.check, "cargo xtask check");
        assert_eq!(file.protected, texts(&["SPEC.md", "WORDS.md", "xtask/"]));
        let spec = Score {
            name: "spec".to_string(),
            better: Better::Higher,
        };
        assert_eq!(file.scores, [spec]);
        assert_eq!(file.planner.on, Some(false));
    }

    // spec(Format of the Lira file): Lira reads these forms of TOML in the Lira file: a key with a text, a number, `true` or `false`, or a list; a table; and a list of tables.
    #[test]
    fn reads_each_form_of_toml_that_the_format_has() {
        let file = read(
            "check = \"make check\" # a text\nagents = 2\nprotected = [\n  'docs/',\n]\n\n[planner]\non = true\n\n[[score]]\nname = \"size\"\nbetter = \"lower\"\n",
        );
        assert_eq!(file.check, "make check");
        assert_eq!(file.agents, Some(2));
        assert_eq!(file.protected, texts(&["docs/"]));
        assert_eq!(file.planner.on, Some(true));
        assert_eq!(file.scores[0].better, Better::Lower);
    }

    // spec(Format of the Lira file): Another form of TOML in the Lira file is an error.
    #[test]
    fn another_form_of_toml_is_an_error() {
        let error = error_of("check = \"x\"\nmodels = { agent = \"a\" }\n");
        assert!(error.starts_with("lira.toml:2: "), "{error}");
        assert!(
            error.contains("form of TOML that the Lira file can't have"),
            "{error}"
        );
        let error = error_of("check = \"\"\"\nmake\n\"\"\"\n");
        assert!(error.starts_with("lira.toml:1: "), "{error}");
    }

    // spec(Format of the Lira file): A Lira file with an unknown key is an error.
    #[test]
    fn an_unknown_key_is_an_error() {
        let error = error_of("check = \"x\"\nchek = \"y\"\n");
        assert!(
            error.starts_with("lira.toml:2: unknown key `chek`"),
            "{error}"
        );
        for (text, key) in [
            ("models.agnt = \"a\"\n", "models.agnt"),
            ("[reviewer]\nharness = [\"a\"]\n", "reviewer.harness"),
            ("[planner]\noff = true\n", "planner.off"),
            (
                "[[score]]\nname = \"a\"\nbetter = \"lower\"\nunit = \"ms\"\n",
                "score.unit",
            ),
        ] {
            let error = error_of(&format!("{CHECK}{text}"));
            assert!(error.contains(&format!("unknown key `{key}`")), "{error}");
        }
    }

    // spec(Format of the Lira file): A Lira file with no `check` is an error.
    #[test]
    fn no_check_is_an_error() {
        let error = error_of("setup = \"make deps\"\n");
        assert!(
            error.starts_with("lira.toml: there is no `check`"),
            "{error}"
        );
        assert!(error_of("").contains("there is no `check`"));
    }

    // A row of the table of keys is a promise about what Lira does with the
    // key. The tests below quote a row only when Lira does that already. The
    // other rows have a test for the type of the key, and no quote yet.

    // spec(Format of the Lira file): | `check` | Text | The check command. It is required. |
    #[test]
    fn check_is_a_text() {
        assert_eq!(read(CHECK).check, "cargo test");
        let error = error_of("check = [\"cargo\", \"test\"]\n");
        assert!(
            error.starts_with("lira.toml:1: `check` must be a text"),
            "{error}"
        );
    }

    #[test]
    fn setup_is_a_text() {
        assert_eq!(read(CHECK).setup, None);
        let file = read(&format!("{CHECK}setup = \"make deps\"\n"));
        assert_eq!(file.setup.as_deref(), Some("make deps"));
        let error = error_of(&format!("{CHECK}setup = true\n"));
        assert!(
            error.starts_with("lira.toml:2: `setup` must be a text"),
            "{error}"
        );
    }

    // spec(Format of the Lira file): | `protected` | List of text | The protected paths. |
    #[test]
    fn protected_is_a_list_of_text() {
        assert_eq!(read(CHECK).protected, texts(&[]));
        let file = read(&format!("{CHECK}protected = [\"SPEC.md\", \"docs/\"]\n"));
        assert_eq!(file.protected, texts(&["SPEC.md", "docs/"]));
        for wrong in ["\"SPEC.md\"", "[1]"] {
            let error = error_of(&format!("{CHECK}protected = {wrong}\n"));
            assert!(
                error.contains("`protected` must be a list of texts"),
                "{error}"
            );
        }
    }

    #[test]
    fn agents_is_a_number() {
        assert_eq!(read(CHECK).agents, None);
        assert_eq!(read(&format!("{CHECK}agents = 2\n")).agents, Some(2));
        for wrong in ["\"2\"", "-1"] {
            let error = error_of(&format!("{CHECK}agents = {wrong}\n"));
            assert!(error.contains("`agents` must be a number"), "{error}");
        }
    }

    #[test]
    fn each_model_is_a_text() {
        assert_eq!(read(CHECK).models, Models::default());
        let file = read(&format!(
            "{CHECK}models.agent = \"a\"\nmodels.planner = \"p\"\nmodels.reviewer = \"r\"\n"
        ));
        let models = Models {
            agent: Some("a".to_string()),
            planner: Some("p".to_string()),
            reviewer: Some("r".to_string()),
        };
        assert_eq!(file.models, models);
        let with_header = read(&format!(
            "{CHECK}[models]\nagent = \"a\"\nplanner = \"p\"\nreviewer = \"r\"\n"
        ));
        assert_eq!(with_header.models, models);
        for key in ["agent", "planner", "reviewer"] {
            let error = error_of(&format!("{CHECK}models.{key} = 5\n"));
            assert!(
                error.contains(&format!("`models.{key}` must be a text")),
                "{error}"
            );
        }
        let error = error_of(&format!("{CHECK}models = \"a\"\n"));
        assert!(error.contains("`models` must be a table"), "{error}");
    }

    #[test]
    fn the_harnesses_of_the_reviewer_are_a_list_of_text_in_order() {
        assert_eq!(read(CHECK).reviewer.harnesses, texts(&[]));
        let file = read(&format!("{CHECK}reviewer.harnesses = [\"b\", \"a\"]\n"));
        assert_eq!(file.reviewer.harnesses, texts(&["b", "a"]));
        let error = error_of(&format!("{CHECK}reviewer.harnesses = \"b\"\n"));
        assert!(
            error.contains("`reviewer.harnesses` must be a list of texts"),
            "{error}"
        );
    }

    // spec(Format of the Lira file): | `score` | List of tables | The scores. Each has a `name` and a `better`. |
    #[test]
    fn score_is_a_list_of_tables_with_a_name_and_a_better() {
        assert_eq!(read(CHECK).scores, []);
        let file = read(&format!(
            "{CHECK}[[score]]\nname = \"spec\"\nbetter = \"higher\"\n\n[[score]]\nbetter = \"lower\"\nname = \"size\"\n"
        ));
        let names: Vec<&str> = file.scores.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["spec", "size"]);
        let error = error_of(&format!("{CHECK}score = [\"spec\"]\n"));
        assert!(
            error.contains("`score` must be a list of tables"),
            "{error}"
        );
        let error = error_of(&format!("{CHECK}[[score]]\nname = \"spec\"\n"));
        assert!(
            error.starts_with("lira.toml:2: this score has no `better`"),
            "{error}"
        );
        let two = "[[score]]\nname = \"a\"\nbetter = \"lower\"\n[[score]]\n";
        let error = error_of(&format!("{CHECK}{two}"));
        assert!(
            error.starts_with("lira.toml:5: this score has no `name`"),
            "{error}"
        );
        let error = error_of(&format!("{CHECK}[[score]]\nbetter = \"lower\"\n"));
        assert!(error.contains("this score has no `name`"), "{error}");
    }

    #[test]
    fn planner_on_is_true_or_false() {
        assert_eq!(read(CHECK).planner.on, None);
        assert_eq!(
            read(&format!("{CHECK}planner.on = true\n")).planner.on,
            Some(true)
        );
        assert_eq!(
            read(&format!("{CHECK}[planner]\non = false\n")).planner.on,
            Some(false)
        );
        let error = error_of(&format!("{CHECK}planner.on = \"true\"\n"));
        assert!(
            error.contains("`planner.on` must be `true` or `false`"),
            "{error}"
        );
    }

    #[test]
    fn the_instructions_of_the_planner_are_a_text() {
        assert_eq!(read(CHECK).planner.instructions, None);
        let file = read(&format!(
            "{CHECK}planner.instructions = \"docs/planner.md\"\n"
        ));
        assert_eq!(
            file.planner.instructions.as_deref(),
            Some("docs/planner.md")
        );
        let error = error_of(&format!("{CHECK}planner.instructions = [\"a\"]\n"));
        assert!(
            error.contains("`planner.instructions` must be a text"),
            "{error}"
        );
    }

    // spec(Format of the Lira file): The value of `better` is `higher` or `lower`.
    #[test]
    fn better_is_higher_or_lower() {
        let score = |better: &str| format!("{CHECK}[[score]]\nname = \"a\"\nbetter = {better}\n");
        assert_eq!(read(&score("\"higher\"")).scores[0].better, Better::Higher);
        assert_eq!(read(&score("\"lower\"")).scores[0].better, Better::Lower);
        for wrong in ["\"more\"", "\"Higher\"", "true"] {
            let error = error_of(&score(wrong));
            assert!(
                error.starts_with("lira.toml:4: `better` must be"),
                "{error}"
            );
        }
    }

    // spec(Format of the Lira file): Two scores with the same name are an error.
    #[test]
    fn two_scores_with_the_same_name_are_an_error() {
        let error = error_of(&format!(
            "{CHECK}[[score]]\nname = \"a\"\nbetter = \"lower\"\n\n[[score]]\nname = \"a\"\nbetter = \"higher\"\n"
        ));
        assert!(
            error.starts_with("lira.toml:7: two scores have the name `a`"),
            "{error}"
        );
    }

    // spec(Format of the Lira file): A path in `protected` that ends with `/` is a folder, and it covers all that is in it.
    #[test]
    fn a_protected_path_that_ends_with_a_slash_covers_its_folder() {
        let file = read(&format!("{CHECK}protected = [\"docs/\", \"notes\"]\n"));
        assert!(file.protects("docs/a.md"));
        assert!(file.protects("docs/deep/b.md"));
        assert!(!file.protects("docs.md"));
        assert!(!file.protects("src/docs/a.md"));
        // With no `/` at its end, a path is one file.
        assert!(file.protects("notes"));
        assert!(!file.protects("notes/a.md"));
    }

    // spec(Format of the Lira file): A path in `protected` is relative to the root of the repository.
    #[test]
    fn a_protected_path_is_relative_to_the_root() {
        let file = read(&format!("{CHECK}protected = [\"SPEC.md\", \"xtask/\"]\n"));
        assert!(file.protects("SPEC.md"));
        assert!(!file.protects("docs/SPEC.md"));
        assert!(file.protects("xtask/src/main.rs"));
        assert!(!file.protects("crates/xtask/src/main.rs"));
        for wrong in ["/SPEC.md", "./docs/", "docs/../SPEC.md", "docs//a.md", ""] {
            let error = error_of(&format!("{CHECK}protected = [\"{wrong}\"]\n"));
            let start =
                format!("lira.toml:2: `{wrong}` in `protected` must be a path from the root");
            assert!(error.starts_with(&start), "{error}");
        }
    }

    // spec(Protected paths): `lira.toml` is always a protected path.
    // spec(Protected paths): `.github/` is always a protected path.
    #[test]
    fn the_lira_file_and_the_github_folder_are_always_protected() {
        let file = read(CHECK);
        assert!(file.protects("lira.toml"));
        assert!(file.protects(".github/workflows/check.yml"));
        assert!(!file.protects("README.md"));
        assert!(!file.protects("crates/lira/lira.toml"));
    }
}
