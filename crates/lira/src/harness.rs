//! A harness as a child process. Lira talks to it through the Agent Client
//! Protocol (ACP): one JSON message on each line of its input and its output.
//!
//! The protocol has its own words, and they stay in the messages. Its
//! "session" is a conversation here, and its "prompt" is a message.
//!
//! Lira sends one request at a time, and reads until the answer comes. It
//! has no time limit: an agent can work on one message for hours.
//!
//! One thread reads the output of the harness all the time, and another
//! writes to its input. So the harness never waits for Lira while Lira
//! writes to it, and Lira never waits in a write for a harness that ended.
//! The harness runs in a process group of its own, so that Lira can stop it
//! with its children. A small process guards it: when Lira ends with no
//! time to stop the harness, the guard stops it.

use crate::json::{self, Json};
use std::fmt;
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// The version of the protocol that Lira speaks.
const VERSION: i64 = 1;

/// The code of JSON-RPC for a request that the receiver does not know.
const NOT_KNOWN: i64 = -32601;

/// How much of what the harness wrote to its error output Lira keeps, in
/// bytes. It is the end of the output.
const KEPT: usize = 4000;

/// How long Lira waits for a harness to end, after the harness lost its
/// input or closed its output.
const WAIT: Duration = Duration::from_secs(2);

/// How often Lira looks if the harness still runs, while it waits for a
/// line.
const LOOK: Duration = Duration::from_millis(100);

/// How long Lira still waits for a line after the harness ended. What the
/// harness wrote before it ended is read in this time.
const LAST: Duration = Duration::from_millis(300);

/// How long Lira reads at most after the harness ended. A child of the
/// harness that Lira could not stop can write for as long as it runs.
const AFTER: Duration = Duration::from_secs(10);

/// The longest line of the output that Lira reads, in bytes. A longer line
/// is an error, so that no harness can make Lira keep a line with no end.
const LONGEST: u64 = 64 * 1024 * 1024;

/// A line of the output, or the sign that a line was longer than `LONGEST`.
/// After that sign, Lira reads no more from the harness.
enum Line {
    Text(Vec<u8>),
    TooLong,
}

/// How many lines of the output Lira keeps while no request reads them. A
/// harness that writes more waits until Lira reads.
const LINES: usize = 1000;

/// A harness that runs.
pub struct Harness {
    program: String,
    child: Child,
    /// The lines that Lira writes to the input. `None` after Lira closed
    /// the input.
    input: Option<Sender<String>>,
    /// It gets a value when a line could not be written to the input.
    input_failed: Receiver<()>,
    /// The lines of the output. It ends when the output ends.
    lines: Receiver<Line>,
    /// When Lira saw that the harness ended, if it did.
    ended_at: Option<Instant>,
    /// When Lira last looked if the harness still runs.
    looked: Instant,
    /// If Lira sent the signal to the process group of the harness.
    group_stopped: bool,
    /// A process that stops the harness and its children when Lira ends
    /// with no time to do it, as after Ctrl-C. `None` after the harness
    /// ended.
    guard: Option<Child>,
    /// The end of what the harness wrote to its error output.
    errors: Arc<Mutex<Vec<u8>>>,
    /// It gets a value when the error output ends.
    errors_ended: Receiver<()>,
    /// The number of the next request.
    next: i64,
    /// How the harness went away, when it did. Lira asks it nothing more.
    gone: Option<String>,
}

/// What the agent said to one message.
#[derive(Debug, PartialEq, Eq)]
pub struct Reply {
    /// The text of the agent. A blank line is between two messages.
    pub text: String,
    pub stop: Stop,
}

/// Why the agent stopped.
#[derive(Debug, PartialEq, Eq)]
pub enum Stop {
    /// The agent is done with the message.
    End,
    /// The AI model reached its limit of tokens.
    MaxTokens,
    /// The agent reached its limit of requests to the AI model.
    MaxRequests,
    /// The AI model refused to continue.
    Refusal,
    /// The message was stopped.
    Cancelled,
    /// A reason that this version of Lira does not know.
    Other(String),
}

impl Stop {
    fn read(reason: &str) -> Stop {
        match reason {
            "end_turn" => Stop::End,
            "max_tokens" => Stop::MaxTokens,
            "max_turn_requests" => Stop::MaxRequests,
            "refusal" => Stop::Refusal,
            "cancelled" => Stop::Cancelled,
            other => Stop::Other(other.to_string()),
        }
    }
}

/// What an agent asks before it does something.
#[derive(Debug)]
pub struct Permission {
    /// What the agent wants to do, in words. It can be empty.
    pub title: String,
    /// The kind of the tool, such as `edit` or `execute`. It can be empty.
    pub kind: String,
    /// The whole tool call, as the harness sent it.
    pub call: Json,
}

#[derive(Debug)]
pub enum Error {
    /// The harness did not start. `reason` says why.
    Start { program: String, reason: String },
    /// The harness went away before it answered.
    Ended {
        program: String,
        method: String,
        /// How it went away, such as `ended (exit status: 1)`.
        how: String,
        /// The end of what it wrote to its error output.
        errors: String,
    },
    /// The harness wrote something that the protocol does not allow.
    Protocol { program: String, problem: String },
    /// The harness answered a request with an error.
    Refused {
        program: String,
        method: String,
        code: i64,
        message: String,
        data: Option<Json>,
    },
    /// The folder of a conversation has a name that is not UTF-8, so no
    /// message can hold it.
    Folder(String),
    /// The harness did not take the AI model that Lira named. `reason` is
    /// what the harness said, when it said something.
    Model {
        program: String,
        model: String,
        reason: Option<String>,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::Start { reason, .. } => f.write_str(reason),
            Error::Ended {
                program,
                method,
                how,
                errors,
            } => {
                write!(
                    f,
                    "the harness `{program}` {how} before it answered `{method}`"
                )?;
                if !errors.is_empty() {
                    write!(f, ". It wrote:\n{errors}")?;
                }
                Ok(())
            }
            Error::Protocol { program, problem } => write!(
                f,
                "the harness `{program}` does not follow the Agent Client Protocol: {problem}"
            ),
            Error::Refused {
                program,
                method,
                code,
                message,
                data,
            } => {
                write!(
                    f,
                    "the harness `{program}` answered `{method}` with error {code}: {message}"
                )?;
                if let Some(data) = data {
                    write!(f, "\n{data}")?;
                }
                Ok(())
            }
            Error::Model {
                program,
                model,
                reason: Some(reason),
            } => write!(
                f,
                "the harness `{program}` does not take the AI model `{model}`: {reason}"
            ),
            Error::Model {
                program,
                model,
                reason: None,
            } => write!(
                f,
                "the harness `{program}` has no option for the AI model, so Lira can't give it the AI model `{model}`"
            ),
            Error::Folder(folder) => write!(
                f,
                "the folder `{folder}` has a name that is not UTF-8, so a harness can't work in it"
            ),
        }
    }
}

impl std::error::Error for Error {}

/// What Lira needs while an agent works on one message.
struct Work<'a> {
    /// The conversation of the message. A harness can have more than one.
    conversation: &'a str,
    permit: &'a mut dyn FnMut(&Permission) -> bool,
    /// What Lira does each time the agent finished a tool call.
    after_tool: &'a mut dyn FnMut(),
    text: String,
    /// The `messageId` of the newest part of the text.
    message: Option<Json>,
}

impl Harness {
    /// Starts the harness, and agrees with it on the version of the
    /// protocol. The command says in which folder and with which variables
    /// the harness runs.
    pub fn start(mut command: Command) -> Result<Harness, Error> {
        let program = command.get_program().to_string_lossy().into_owned();
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        let mut child = command.spawn().map_err(|error| {
            // The system gives the same error for a program that is not
            // there, for a program that needs another one that is not
            // there, and for a folder that is not there.
            let no_folder = command
                .get_current_dir()
                .filter(|folder| matches!(folder.try_exists(), Ok(false)));
            let reason = match (no_folder, error.kind()) {
                (Some(folder), _) => format!(
                    "the harness `{program}` did not start: the folder `{}` is not there",
                    folder.display()
                ),
                (None, ErrorKind::NotFound) => format!(
                    "Lira can't find the harness `{program}` on this host, or a program that it needs to start"
                ),
                (None, _) => format!("the harness `{program}` did not start: {error}"),
            };
            Error::Start {
                program: program.clone(),
                reason,
            }
        })?;
        // Lira starts no harness that it can't stop when Lira itself is
        // stopped.
        let guard = match guard(child.id()) {
            Ok(guard) => Some(guard),
            Err(error) => {
                // This stops the harness, and not what the harness started
                // in these first moments: Lira stops a process group with
                // `sh`, and `sh` is what did not start here.
                let _ = child.kill();
                let _ = child.wait();
                return Err(Error::Start {
                    reason: format!(
                        "Lira can't start the process that guards the harness `{program}`. It is a `sh`, and it did not start: {error}"
                    ),
                    program,
                });
            }
        };
        let (input, input_failed) = write_lines(child.stdin.take().expect("the input is a pipe"));
        let lines = read_lines(child.stdout.take().expect("the output is a pipe"));
        let (errors, errors_ended) = keep(child.stderr.take().expect("the error output is a pipe"));
        let mut harness = Harness {
            program,
            child,
            input: Some(input),
            input_failed,
            lines,
            ended_at: None,
            looked: Instant::now(),
            group_stopped: false,
            guard,
            errors,
            errors_ended,
            next: 1,
            gone: None,
        };

        let params = Json::object([
            ("protocolVersion", Json::number(VERSION)),
            (
                "clientCapabilities",
                Json::object([
                    (
                        "fs",
                        Json::object([
                            ("readTextFile", Json::Bool(false)),
                            ("writeTextFile", Json::Bool(false)),
                        ]),
                    ),
                    ("terminal", Json::Bool(false)),
                ]),
            ),
            (
                "clientInfo",
                Json::object([
                    ("name", Json::text("lira")),
                    ("version", Json::text(env!("CARGO_PKG_VERSION"))),
                ]),
            ),
        ]);
        let result = harness.request("initialize", params, None)?;
        match result.get("protocolVersion").and_then(Json::as_i64) {
            Some(VERSION) => Ok(harness),
            Some(other) => Err(harness.protocol(format!(
                "it speaks version {other}, and Lira speaks version {VERSION}"
            ))),
            None => Err(harness.protocol(format!(
                "its answer to `initialize` has no `protocolVersion`: {result}"
            ))),
        }
    }

    /// Starts a conversation with an agent that works in this folder, and
    /// gives the name of the conversation. With `model`, the agent of the
    /// conversation has that AI model, or the conversation is an error.
    pub fn converse(&mut self, folder: &Path, model: Option<&str>) -> Result<String, Error> {
        let folder = std::path::absolute(folder).unwrap_or_else(|_| folder.to_path_buf());
        let Some(folder) = folder.to_str() else {
            return Err(Error::Folder(folder.to_string_lossy().into_owned()));
        };
        let params = Json::object([
            ("cwd", Json::text(folder)),
            ("mcpServers", Json::List(Vec::new())),
        ]);
        let result = self.request("session/new", params, None)?;
        let Some(name) = result.get("sessionId").and_then(Json::as_text) else {
            return Err(self.protocol(format!(
                "its answer to `session/new` has no `sessionId`: {result}"
            )));
        };
        if let Some(model) = model {
            self.choose_model(name, model, &result)?;
        }
        Ok(name.to_string())
    }

    /// Gives a conversation its AI model. A harness says in its answer to
    /// `session/new` which options a conversation has, and one of them can
    /// be for the AI model. `new` is that answer.
    fn choose_model(&mut self, conversation: &str, model: &str, new: &Json) -> Result<(), Error> {
        let refused = |program: &str, reason: Option<String>| Error::Model {
            program: program.to_string(),
            model: model.to_string(),
            reason,
        };
        let options = new.get("configOptions").and_then(Json::as_list);
        let option = options.into_iter().flatten().find_map(|option| {
            let for_the_model = option.get("category").and_then(Json::as_text) == Some("model");
            option.get("id").filter(|_| for_the_model)
        });
        let Some(option) = option else {
            return Err(refused(&self.program, None));
        };
        let params = Json::object([
            ("sessionId", Json::text(conversation)),
            ("configId", option.clone()),
            ("value", Json::text(model)),
        ]);
        // The harness knows which names it takes. Lira does not look in the
        // list of the option: a harness can take more names than it lists.
        match self.request("session/set_config_option", params, None) {
            Ok(_) => Ok(()),
            Err(Error::Refused { message, .. }) => Err(refused(&self.program, Some(message))),
            Err(error) => Err(error),
        }
    }

    /// Gives a message to the agent of a conversation, and waits for the
    /// reply. `permit` answers each permission request of the agent: `true`
    /// is yes, and `false` is no.
    pub fn send(
        &mut self,
        conversation: &str,
        message: &str,
        permit: &mut dyn FnMut(&Permission) -> bool,
    ) -> Result<Reply, Error> {
        self.send_and_watch(conversation, message, permit, &mut || {})
    }

    /// The same as `send`. Lira also calls `after_tool` each time the agent
    /// finished a tool call, with success or with a failure. Lira reads
    /// nothing from the harness while `after_tool` runs, so a permission
    /// request waits for it. Work that takes long belongs on another
    /// thread.
    pub fn send_and_watch(
        &mut self,
        conversation: &str,
        message: &str,
        permit: &mut dyn FnMut(&Permission) -> bool,
        after_tool: &mut dyn FnMut(),
    ) -> Result<Reply, Error> {
        let params = Json::object([
            ("sessionId", Json::text(conversation)),
            (
                "prompt",
                Json::List(vec![Json::object([
                    ("type", Json::text("text")),
                    ("text", Json::text(message)),
                ])]),
            ),
        ]);
        let mut work = Work {
            conversation,
            permit,
            after_tool,
            text: String::new(),
            message: None,
        };
        let result = self.request("session/prompt", params, Some(&mut work))?;
        match result.get("stopReason").and_then(Json::as_text) {
            Some(reason) => Ok(Reply {
                text: work.text,
                stop: Stop::read(reason),
            }),
            None => Err(self.protocol(format!(
                "its answer to `session/prompt` has no `stopReason`: {result}"
            ))),
        }
    }

    /// Sends one request, and reads until its answer comes. It handles what
    /// the harness sends before the answer.
    fn request(
        &mut self,
        method: &str,
        params: Json,
        mut work: Option<&mut Work>,
    ) -> Result<Json, Error> {
        let number = self.next;
        self.next += 1;
        self.write(
            method,
            &Json::object([
                ("jsonrpc", Json::text("2.0")),
                ("id", Json::number(number)),
                ("method", Json::text(method)),
                ("params", params),
            ]),
        )?;
        loop {
            let message = self.read(method)?;
            let theirs = message.get("method").and_then(Json::as_text);
            match (theirs, message.get("id")) {
                // A request of the harness.
                (Some(theirs), Some(id)) => {
                    if !matches!(id, Json::Null | Json::Number(_) | Json::Text(_)) {
                        return Err(self.protocol(format!(
                            "the `id` of a request is not a number and not a text: {message}"
                        )));
                    }
                    let answer = match (theirs, work.as_deref_mut()) {
                        ("session/request_permission", Some(work)) => {
                            // Lira allows nothing for a conversation that
                            // it does not wait for.
                            let outcome = if work.is_about(&message) {
                                match Harness::permission(&message, work) {
                                    Ok(outcome) => outcome,
                                    Err(problem) => return Err(self.protocol(problem)),
                                }
                            } else {
                                Json::object([("outcome", Json::text("cancelled"))])
                            };
                            ("result", Json::object([("outcome", outcome)]))
                        }
                        _ => (
                            "error",
                            Json::object([
                                ("code", Json::number(NOT_KNOWN)),
                                (
                                    "message",
                                    Json::text(&format!("Lira does not answer `{theirs}` here")),
                                ),
                            ]),
                        ),
                    };
                    self.write(
                        method,
                        &Json::object([("jsonrpc", Json::text("2.0")), ("id", id.clone()), answer]),
                    )?;
                }
                // A note of the harness. Lira reads the text of the agent
                // and the end of each tool call, and nothing else yet.
                (Some(theirs), None) => {
                    if let (Some(work), "session/update") = (work.as_deref_mut(), theirs)
                        && work.is_about(&message)
                    {
                        work.update(&message);
                    }
                }
                // An answer.
                (None, Some(id)) => {
                    if id.as_i64() != Some(number) {
                        return Err(self.protocol(format!(
                            "it answered request {id}, and Lira waits for the answer to request {number}"
                        )));
                    }
                    // An answer has a result, or an error with a code and
                    // a message.
                    if let Some(error) = message.get("error").filter(|e| **e != Json::Null) {
                        let code = error.get("code").and_then(Json::as_i64);
                        let text = error.get("message").and_then(Json::as_text);
                        let (Some(code), Some(text)) = (code, text) else {
                            return Err(self.protocol(format!(
                                "the error in its answer to `{method}` has no `code` or no `message`: {message}"
                            )));
                        };
                        return Err(Error::Refused {
                            program: self.program.clone(),
                            method: method.to_string(),
                            code,
                            message: text.to_string(),
                            data: error.get("data").cloned(),
                        });
                    }
                    return match message.get("result") {
                        Some(result) => Ok(result.clone()),
                        None => Err(self.protocol(format!(
                            "its answer to `{method}` has no `result` and no `error`: {message}"
                        ))),
                    };
                }
                (None, None) => {
                    return Err(
                        self.protocol(format!("a message has no `method` and no `id`: {message}"))
                    );
                }
            }
        }
    }

    /// Asks `permit`, and gives the outcome for the answer. A request that
    /// the protocol does not allow is an error, and `permit` is not asked.
    fn permission(message: &Json, work: &mut Work) -> Result<Json, String> {
        let params = message.get("params");
        let call = params.and_then(|params| params.get("toolCall"));
        let options = params
            .and_then(|params| params.get("options"))
            .and_then(Json::as_list);
        let (Some(call @ Json::Object(_)), Some(options)) = (call, options) else {
            return Err(format!(
                "a permission request has no `toolCall` or no `options`: {message}"
            ));
        };
        // Each option has a name that Lira can give back, and a kind.
        let mut offered = Vec::new();
        for option in options {
            let name = option
                .get("optionId")
                .filter(|name| name.as_text().is_some());
            let kind = option.get("kind").and_then(Json::as_text);
            let (Some(name), Some(kind)) = (name, kind) else {
                return Err(format!(
                    "an option of a permission request has no `optionId` or no `kind`: {message}"
                ));
            };
            offered.push((kind, name));
        }
        let text = |key: &str| {
            call.get(key)
                .and_then(Json::as_text)
                .unwrap_or("")
                .to_string()
        };
        let permission = Permission {
            title: text("title"),
            kind: text("kind"),
            call: call.clone(),
        };
        // An option for one time is better than an option for always: with
        // "always", the agent does not ask the next time.
        let kinds = if (work.permit)(&permission) {
            ["allow_once", "allow_always"]
        } else {
            ["reject_once", "reject_always"]
        };
        let option = kinds.iter().find_map(|wanted| {
            offered
                .iter()
                .find(|(kind, _)| kind == wanted)
                .map(|(_, name)| *name)
        });
        // The protocol does not say that a request has an option of each
        // kind. With no option for the answer, Lira chooses none, and the
        // agent does not do what it asked for.
        Ok(match option {
            Some(option) => Json::object([
                ("outcome", Json::text("selected")),
                ("optionId", option.clone()),
            ]),
            None => Json::object([("outcome", Json::text("cancelled"))]),
        })
    }

    /// Gives a line to the thread that writes to the input. The thread
    /// can wait for a harness that does not read, and Lira does not: it goes
    /// on to read, and there it sees a harness that ended.
    fn write(&mut self, method: &str, message: &Json) -> Result<(), Error> {
        if self.gone.is_some() {
            return Err(self.ended(method, None));
        }
        let sent = match &self.input {
            Some(input) => input.send(format!("{message}\n")).is_ok(),
            None => false,
        };
        if sent {
            Ok(())
        } else {
            Err(self.ended(method, Some("closed its input")))
        }
    }

    /// Reads the next message. It skips empty lines.
    fn read(&mut self, method: &str) -> Result<Json, Error> {
        loop {
            let line = self.line(method)?;
            let Ok(line) = std::str::from_utf8(&line) else {
                return Err(self.protocol(format!(
                    "a line is not UTF-8: {}",
                    String::from_utf8_lossy(&line)
                )));
            };
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let message = match json::read(line) {
                Ok(message @ Json::Object(_)) => message,
                Ok(_) => return Err(self.protocol(format!("a line is not an object: {line}"))),
                Err(error) => {
                    return Err(self.protocol(format!("a line is not JSON ({error}): {line}")));
                }
            };
            if message.get("jsonrpc").and_then(Json::as_text) != Some("2.0") {
                return Err(self.protocol(format!(
                    "a message has no `jsonrpc` with the value `2.0`: {line}"
                )));
            }
            return Ok(message);
        }
    }

    /// Waits for the next line of the output.
    ///
    /// The output can stay open after the harness ended, when a child of
    /// the harness holds it, and that child can write on. So Lira looks by
    /// the clock if the harness still runs, also while lines come.
    ///
    /// After the harness ended, Lira still reads what the harness wrote
    /// before. It stops when no line comes for a short time, or after
    /// `AFTER` at the latest. The time that Lira needs for a line that it
    /// got does not count as a time with no line.
    fn line(&mut self, method: &str) -> Result<Vec<u8>, Error> {
        if self.gone.is_some() {
            return Err(self.ended(method, None));
        }
        loop {
            if self.ended_at.is_none() && self.looked.elapsed() >= LOOK {
                self.looked = Instant::now();
                if !matches!(self.child.try_wait(), Ok(None)) {
                    self.ended_at = Some(Instant::now());
                    // The number of the group is still the harness's own
                    // at this moment, so this is the time for the signal.
                    self.stop_group();
                }
            }
            if self.input_failed.try_recv().is_ok() {
                return Err(self.ended(method, Some("closed its input")));
            }
            let wait = match self.ended_at {
                Some(at) if at.elapsed() >= AFTER => {
                    return Err(self.ended(method, Some("ended")));
                }
                Some(_) => LAST,
                None => LOOK,
            };
            match self.lines.recv_timeout(wait) {
                Ok(Line::Text(line)) => return Ok(line),
                Ok(Line::TooLong) => {
                    // Lira reads no more, so the harness can't go on.
                    self.stop();
                    self.gone = Some("wrote a line that is too long, and Lira stopped it".into());
                    return Err(self.protocol(format!("a line is longer than {LONGEST} bytes")));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(self.ended(method, Some("closed its output")));
                }
                Err(RecvTimeoutError::Timeout) if self.ended_at.is_some() => {
                    return Err(self.ended(method, Some("ended")));
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
        }
    }

    fn protocol(&self, problem: String) -> Error {
        Error::Protocol {
            program: self.program.clone(),
            problem,
        }
    }

    /// The error for a harness that went away. `did` is what Lira saw, the
    /// first time. Lira stops the harness and its children, so that nothing
    /// of it stays, and asks it nothing more.
    fn ended(&mut self, method: &str, did: Option<&str>) -> Error {
        if let Some(did) = did
            && self.gone.is_none()
        {
            let how = match self.stop() {
                Some(status) => format!("ended ({status})"),
                None => format!("{did}, and Lira stopped it"),
            };
            self.gone = Some(how);
            // The error output ends when the harness and its children are
            // gone.
            let _ = self.errors_ended.recv_timeout(WAIT);
        }
        let errors = self
            .errors
            .lock()
            .map(|kept| kept.clone())
            .unwrap_or_default();
        Error::Ended {
            program: self.program.clone(),
            method: method.to_string(),
            how: self.gone.clone().unwrap_or_default(),
            errors: String::from_utf8_lossy(&errors).trim().to_string(),
        }
    }

    /// Closes the input of the harness, and waits a short time for it to
    /// end. Then it stops what still runs: the harness, and each child that
    /// the harness left in its process group. It returns how the harness
    /// ended, when it ended without Lira.
    ///
    /// A child that left the process group stays. Lira has no way to find
    /// it.
    fn stop(&mut self) -> Option<ExitStatus> {
        self.input = None;
        let started = Instant::now();
        let status = loop {
            match self.child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if started.elapsed() < WAIT => thread::sleep(Duration::from_millis(10)),
                _ => break None,
            }
        };
        self.stop_group();
        if status.is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        status
    }

    /// Stops each process in the process group of the harness, one time.
    ///
    /// The number of the group is the number of the harness. `kill` of the
    /// shell is the way to it with no code that Rust can't check. Lira sends
    /// the signal while the harness runs, or at once when it sees that the
    /// harness ended. Later, the system could give the number to a process
    /// that is not of the harness.
    fn stop_group(&mut self) {
        if self.group_stopped {
            return;
        }
        self.group_stopped = true;
        // Lira sends the signal itself now, so the guard is not needed. It
        // goes first: after this, the number of the group can belong to
        // other processes.
        if let Some(mut guard) = self.guard.take() {
            let _ = guard.kill();
            let _ = guard.wait();
        }
        let group = format!("-{}", self.child.id());
        let _ = Command::new("sh")
            .args(["-c", "kill -s KILL -- \"$1\"", "sh", &group])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

impl Work<'_> {
    /// If a message of the harness is about the conversation of this work.
    fn is_about(&self, message: &Json) -> bool {
        message
            .get("params")
            .and_then(|params| params.get("sessionId"))
            .and_then(Json::as_text)
            == Some(self.conversation)
    }

    /// Keeps the text of an update that is a part of what the agent says,
    /// and calls `after_tool` for an update that ends a tool call.
    fn update(&mut self, message: &Json) {
        let Some(update) = message
            .get("params")
            .and_then(|params| params.get("update"))
        else {
            return;
        };
        let kind = update.get("sessionUpdate").and_then(Json::as_text);
        if matches!(kind, Some("tool_call" | "tool_call_update")) {
            // A tool call ends with one of these two. The harness can name
            // a tool call for the first time when it has ended already.
            let status = update.get("status").and_then(Json::as_text);
            if matches!(status, Some("completed" | "failed")) {
                (self.after_tool)();
            }
            return;
        }
        if kind != Some("agent_message_chunk") {
            return;
        }
        let content = update.get("content");
        let Some(text) = content
            .filter(|content| content.get("type").and_then(Json::as_text) == Some("text"))
            .and_then(|content| content.get("text"))
            .and_then(Json::as_text)
        else {
            return;
        };
        // A message with no name, or with the name `null`, is one message.
        let message = update.get("messageId").filter(|name| **name != Json::Null);
        if !self.text.is_empty() && message != self.message.as_ref() {
            self.text.push_str("\n\n");
        }
        self.message = message.cloned();
        self.text.push_str(text);
    }
}

/// When Lira needs the harness no more, it closes the input of the harness.
/// What does not end then is stopped: the harness and its children.
impl Drop for Harness {
    fn drop(&mut self) {
        if self.gone.is_none() {
            self.stop();
        }
    }
}

/// Starts the process that guards a harness. It reads from a pipe that only
/// Lira holds. The pipe ends when Lira ends, however that happens, and the
/// guard then stops the process group of the harness. It has a process
/// group of its own, so that Ctrl-C in the terminal does not reach it.
fn guard(harness: u32) -> std::io::Result<Child> {
    let wait_then_stop = r#"while IFS= read -r line; do :; done; kill -s KILL -- "$1""#;
    Command::new("sh")
        .args(["-c", wait_then_stop, "sh", &format!("-{harness}")])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
}

/// Writes each line that it gets to the input of a harness, on a thread of
/// its own. The second value gets a value when a line could not be written.
/// The input closes when the first value is dropped.
fn write_lines(mut input: ChildStdin) -> (Sender<String>, Receiver<()>) {
    let (lines, given) = mpsc::channel::<String>();
    let (fail, failed) = mpsc::channel();
    thread::spawn(move || {
        for line in given {
            if input
                .write_all(line.as_bytes())
                .and_then(|()| input.flush())
                .is_err()
            {
                let _ = fail.send(());
                break;
            }
        }
    });
    (lines, failed)
}

/// Reads the output of a harness until it ends, and gives each line. It
/// keeps `LINES` lines at most, and then waits until Lira reads one. It
/// stops at a line that is longer than `LONGEST`.
fn read_lines(output: ChildStdout) -> Receiver<Line> {
    let (found, lines) = mpsc::sync_channel(LINES);
    thread::spawn(move || {
        let mut output = BufReader::new(output);
        loop {
            let mut line = Vec::new();
            // One byte more than the longest line shows a line that is
            // longer.
            let read = (&mut output).take(LONGEST + 1).read_until(b'\n', &mut line);
            let line = match read {
                Ok(0) | Err(_) => break,
                Ok(_) if line.len() as u64 > LONGEST => Line::TooLong,
                Ok(_) => Line::Text(line),
            };
            let stop = matches!(line, Line::TooLong);
            if found.send(line).is_err() || stop {
                break;
            }
        }
    });
    lines
}

/// Reads the error output of a harness until it ends, and keeps the end of
/// it. A harness that writes much there must not wait for Lira.
fn keep(mut output: ChildStderr) -> (Arc<Mutex<Vec<u8>>>, Receiver<()>) {
    let kept = Arc::new(Mutex::new(Vec::new()));
    let (ended, errors_ended) = mpsc::channel();
    let writer = Arc::clone(&kept);
    thread::spawn(move || {
        let mut part = [0; 1024];
        loop {
            match output.read(&mut part) {
                Ok(0) | Err(_) => break,
                Ok(count) => {
                    let Ok(mut kept) = writer.lock() else { break };
                    kept.extend_from_slice(&part[..count]);
                    let extra = kept.len().saturating_sub(KEPT);
                    kept.drain(..extra);
                }
            }
        }
        let _ = ended.send(());
    });
    (kept, errors_ended)
}
