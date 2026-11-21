//! Runs a command to its end, and keeps what it wrote.
//!
//! A command can leave a process behind that holds its output open. Lira
//! waits for the command, not for that process.

use std::io::{self, PipeReader, Read};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// How long Lira still reads what a command wrote, after the command ended.
const LAST: Duration = Duration::from_millis(500);

/// How a command ended, and what it wrote.
pub struct Ended {
    pub status: ExitStatus,
    pub output: Vec<u8>,
    /// What it wrote to its error output. It is empty when the command ran
    /// with the two outputs together.
    pub errors: Vec<u8>,
}

/// Runs the command with no input. With `together`, what the command writes
/// to its error output goes to `output` too, in the order of the writing.
pub fn run(mut command: Command, together: bool) -> io::Result<Ended> {
    let (output, output_end) = io::pipe()?;
    command.stdin(Stdio::null());
    let errors = if together {
        command.stdout(output_end.try_clone()?).stderr(output_end);
        None
    } else {
        let (errors, errors_end) = io::pipe()?;
        command.stdout(output_end).stderr(errors_end);
        Some(errors)
    };
    let mut child = command.spawn()?;
    // The command has the ends for writing. Lira closes its own, so that
    // the reading ends when the last writer does.
    drop(command);
    let output = Keeper::new(output);
    let errors = errors.map(Keeper::new);
    let status = child.wait()?;
    Ok(Ended {
        status,
        output: output.take(),
        errors: errors.map(Keeper::take).unwrap_or_default(),
    })
}

/// Reads a pipe on a thread of its own, and keeps what it reads.
struct Keeper {
    /// `None` after Lira took what was kept. The thread then reads on, so
    /// that a process that still writes does not stop, and keeps nothing.
    kept: Arc<Mutex<Option<Vec<u8>>>>,
    /// It gets a value when the pipe ends.
    ended: Receiver<()>,
}

impl Keeper {
    fn new(mut pipe: PipeReader) -> Keeper {
        let kept = Arc::new(Mutex::new(Some(Vec::new())));
        let (end, ended) = mpsc::channel();
        let writer = Arc::clone(&kept);
        thread::spawn(move || {
            let mut part = [0; 4096];
            loop {
                match pipe.read(&mut part) {
                    Ok(0) | Err(_) => break,
                    Ok(count) => {
                        let Ok(mut kept) = writer.lock() else { break };
                        if let Some(kept) = kept.as_mut() {
                            kept.extend_from_slice(&part[..count]);
                        }
                    }
                }
            }
            let _ = end.send(());
        });
        Keeper { kept, ended }
    }

    /// What the pipe gave until now. It waits a short time for the pipe to
    /// end first.
    fn take(self) -> Vec<u8> {
        let _ = self.ended.recv_timeout(LAST);
        self.kept
            .lock()
            .ok()
            .and_then(|mut kept| kept.take())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::time::Instant;

    fn sh(script: &str) -> Command {
        let mut command = Command::new("sh");
        command.args(["-c", script]);
        command
    }

    #[test]
    fn keeps_the_two_outputs_apart_or_together() {
        let script = "echo one; echo two >&2; echo three; exit 3";
        let apart = run(sh(script), false).unwrap();
        assert_eq!(apart.status.code(), Some(3));
        assert_eq!(apart.output, b"one\nthree\n");
        assert_eq!(apart.errors, b"two\n");
        let together = run(sh(script), true).unwrap();
        assert_eq!(together.output, b"one\ntwo\nthree\n");
        assert_eq!(together.errors, b"");
    }

    #[test]
    fn keeps_more_than_a_pipe_holds() {
        let ended = run(
            sh("head -c 300000 /dev/zero; head -c 300000 /dev/zero >&2"),
            false,
        )
        .unwrap();
        assert_eq!(ended.output.len(), 300_000);
        assert_eq!(ended.errors.len(), 300_000);
    }

    #[test]
    fn does_not_wait_for_a_process_that_the_command_left_behind() {
        let started = Instant::now();
        let ended = run(sh("sleep 20 & echo started"), false).unwrap();
        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(ended.status.success());
        assert_eq!(ended.output, b"started\n");
    }

    #[test]
    fn keeps_nothing_after_lira_took_what_was_kept() {
        let (reader, mut writer) = io::pipe().unwrap();
        let keeper = Keeper::new(reader);
        let kept = Arc::clone(&keeper.kept);
        writer.write_all(b"before").unwrap();
        // The pipe is still open, so this waits the short time.
        assert_eq!(keeper.take(), b"before");
        writer.write_all(b"after").unwrap();
        drop(writer);
        thread::sleep(Duration::from_millis(200));
        assert_eq!(*kept.lock().unwrap(), None);
    }
}
