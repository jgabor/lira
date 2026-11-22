//! Lira: the server and the command line, in one program.

use std::process::ExitCode;

const USAGE: &str = "\
usage: lira <command>

commands:
  run \"<request>\"   run an agent on the request, to a pull request that is ready for review
  check             run the check command of the Lira file, and print each score
  --version         print the version

exit status: 0 success, 1 failure, 2 bad usage
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args[..] {
        ["--version"] => {
            println!("lira {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        ["check"] => check(),
        // Lira has no options for a run. So one word that starts with `-`,
        // such as `--help`, is a mistake, and no agent starts on it. A
        // request with more words can start with `-`.
        ["run", request] if !request.trim().is_empty() && !is_an_option(request) => run(request),
        ["run", ..] => {
            eprint!(
                "usage: lira run \"<request>\"\n\nThe request is one argument. It is not empty, and it is not one word that starts with `-`.\n"
            );
            ExitCode::from(2)
        }
        _ => {
            eprint!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

/// If an argument looks like an option: one word that starts with `-`.
fn is_an_option(argument: &str) -> bool {
    argument.starts_with('-') && !argument.trim().contains(char::is_whitespace)
}

fn check() -> ExitCode {
    let folder = match std::env::current_dir() {
        Ok(folder) => folder,
        Err(e) => return failure(&format!("couldn't find the current folder: {e}")),
    };
    match lira::check::command(&folder) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(message) => failure(&message),
    }
}

fn run(request: &str) -> ExitCode {
    let folder = match std::env::current_dir() {
        Ok(folder) => folder,
        Err(e) => return failure(&format!("couldn't find the current folder: {e}")),
    };
    match lira::run::command(&folder, request) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => failure(&message),
    }
}

fn failure(message: &str) -> ExitCode {
    eprintln!("lira: {message}");
    ExitCode::from(1)
}
