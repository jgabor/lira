//! `cargo xtask check`: the check command of this repository.

use crate::workspace::no_cargo;
use crate::{Problem, Verdict, quotes, testrun, workspace};
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::Path;
use std::process::{Command, Stdio};

/// The variable that names the file for the scores. SPEC.md, "Check".
pub const SCORES: &str = "LIRA_SCORES";

struct Step {
    name: &'static str,
    args: &'static [&'static str],
}

/// The steps that are one cargo command. They run first, in this order.
const STEPS: &[Step] = &[
    Step {
        name: "format",
        args: &["fmt", "--all", "--check"],
    },
    Step {
        name: "lint",
        args: &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--locked",
            "--",
            "-D",
            "warnings",
        ],
    },
];

/// Runs every step, also after one fails, so that one run shows all that is
/// wrong.
pub fn run() -> Result<Verdict, Problem> {
    let root = workspace::root()?;
    let mut passed = true;
    for step in STEPS {
        passed &= record(step.name, cargo_passes(&root, step.args)?);
    }
    let results = testrun::run(&root)?;
    passed &= record("tests", results.all_passed());
    let score = quotes::score(&root, &results)?;
    for finding in &score.findings {
        eprintln!("quotes: {finding}");
    }
    passed &= record("quotes", score.findings.is_empty());
    println!("{}", score_line(score.proven));
    write_score(score.proven)?;
    println!("check: {}", if passed { "pass" } else { "fail" });
    Ok(if passed { Verdict::Pass } else { Verdict::Fail })
}

fn record(name: &str, passed: bool) -> bool {
    println!("{} {name}", if passed { "pass" } else { "fail" });
    passed
}

/// Runs a cargo command, with its output on our stderr, so that stdout
/// carries only the result.
fn cargo_passes(root: &Path, args: &[&str]) -> Result<bool, Problem> {
    let status = Command::new("cargo")
        .args(args)
        .current_dir(root)
        .stdout(Stdio::from(io::stderr()))
        .status()
        .map_err(no_cargo)?;
    Ok(status.success())
}

fn score_line(proven: usize) -> String {
    format!("spec = {proven}")
}

/// Adds the score line to the file that `LIRA_SCORES` names. Without the
/// variable, nothing is written.
fn write_score(proven: usize) -> Result<(), Problem> {
    let Some(path) = std::env::var_os(SCORES).filter(|path| !path.is_empty()) else {
        return Ok(());
    };
    let shown = Path::new(&path).display().to_string();
    let mut file = OpenOptions::new()
        .append(true)
        .create(true)
        .open(&path)
        .map_err(|e| Problem(format!("couldn't open {shown}, which {SCORES} names: {e}")))?;
    writeln!(file, "{}", score_line(proven))
        .map_err(|e| Problem(format!("couldn't write the score to {shown}: {e}")))
}
