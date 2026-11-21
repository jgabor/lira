//! Lira: the server and the command line, in one program.

use std::process::ExitCode;

const USAGE: &str = "usage: lira --version";

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() == 1 && args[0] == "--version" {
        println!("lira {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    eprintln!("{USAGE}");
    ExitCode::from(2)
}
