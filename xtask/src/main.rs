//! The program behind `cargo xtask`.

mod check;
mod quotes;
mod spec;
mod testrun;
mod workspace;

use std::process::ExitCode;

const USAGE: &str = "\
usage: cargo xtask <command>

commands:
  check   run the format test, the lint, the tests, and the spec quotes

exit status: 0 pass, 1 failure, 2 bad usage or a check that could not run
";

/// The result of a command that was able to run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    Fail,
}

/// Why a command couldn't run, which is exit status 2.
#[derive(Debug)]
pub struct Problem(pub String);

fn run(args: &[&str]) -> Result<Verdict, Problem> {
    match args {
        ["check"] => check::run(),
        ["check", extra, ..] => Err(Problem(format!(
            "`check` takes no arguments, but got `{extra}`\n\n{USAGE}"
        ))),
        ["-h" | "--help" | "help"] => {
            print!("{USAGE}");
            Ok(Verdict::Pass)
        }
        [] => Err(Problem(format!("no command given\n\n{USAGE}"))),
        [other, ..] => Err(Problem(format!("unknown command `{other}`\n\n{USAGE}"))),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match run(&args) {
        Ok(Verdict::Pass) => ExitCode::SUCCESS,
        Ok(Verdict::Fail) => ExitCode::from(1),
        Err(Problem(message)) => {
            eprintln!("error: {message}");
            ExitCode::from(2)
        }
    }
}
