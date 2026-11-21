//! Lira: the server and the command line, in one program.

use std::process::ExitCode;

const USAGE: &str = "\
usage: lira <command>

commands:
  check       run the check command of the Lira file, and print each score
  --version   print the version

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
        _ => {
            eprint!("{USAGE}");
            ExitCode::from(2)
        }
    }
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

fn failure(message: &str) -> ExitCode {
    eprintln!("lira: {message}");
    ExitCode::from(1)
}
