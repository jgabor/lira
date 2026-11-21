//! A harness as a child process. Lira talks to it through the Agent Client
//! Protocol (ACP): one JSON message on each line of its input and its output.
//!
//! The protocol has its own words, and they stay in the messages. Its
//! "session" is a conversation here, and its "prompt" is a message.
//!
//! Lira sends one request at a time, and reads until the answer comes. It
//! has no time limit: an agent can work on one message for hours.

use crate::json::{self, Json};
use std::fmt;
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
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

/// A harness that runs.
pub struct Harness {
    program: String,
    child: Child,
    /// `None` after Lira closed it.
    input: Option<ChildStdin>,
    output: BufReader<ChildStdout>,
    /// The end of what the harness wrote to its error output.
    errors: Arc<Mutex<Vec<u8>>>,
    /// It gets a value when the error output ends.
    errors_ended: Receiver<()>,
    /// The number of the next request.
    next: i64,
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
    /// The harness did not start.
    Start { program: String, reason: String },
    /// The harness ended, or closed its output, before it answered.
    Ended {
        program: String,
        method: String,
        /// How it ended, when Lira could see it.
        status: Option<String>,
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
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::Start { program, reason } => {
                if reason.is_empty() {
                    write!(f, "the harness `{program}` is not on this host")
                } else {
                    write!(f, "the harness `{program}` did not start: {reason}")
                }
            }
            Error::Ended {
                program,
                method,
                status,
                errors,
            } => {
                write!(
                    f,
                    "the harness `{program}` ended before it answered `{method}`"
                )?;
                if let Some(status) = status {
                    write!(f, " ({status})")?;
                }
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
    permit: &'a mut dyn FnMut(&Permission) -> bool,
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
            .stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|error| Error::Start {
            program: program.clone(),
            reason: match error.kind() {
                ErrorKind::NotFound => String::new(),
                _ => error.to_string(),
            },
        })?;
        let input = child.stdin.take();
        let output = BufReader::new(child.stdout.take().expect("the output is a pipe"));
        let (errors, errors_ended) = keep(child.stderr.take().expect("the error output is a pipe"));
        let mut harness = Harness {
            program,
            child,
            input,
            output,
            errors,
            errors_ended,
            next: 1,
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
    /// gives the name of the conversation. `extra` is what one harness
    /// understands and the protocol does not, such as the AI model. It is
    /// the `_meta` of the message.
    pub fn converse(&mut self, folder: &Path, extra: Option<Json>) -> Result<String, Error> {
        let folder = std::path::absolute(folder).unwrap_or_else(|_| folder.to_path_buf());
        let Some(folder) = folder.to_str() else {
            return Err(Error::Folder(folder.to_string_lossy().into_owned()));
        };
        let mut params = vec![
            ("cwd".to_string(), Json::text(folder)),
            ("mcpServers".to_string(), Json::List(Vec::new())),
        ];
        if let Some(extra) = extra {
            params.push(("_meta".to_string(), extra));
        }
        let result = self.request("session/new", Json::Object(params), None)?;
        match result.get("sessionId").and_then(Json::as_text) {
            Some(name) => Ok(name.to_string()),
            None => Err(self.protocol(format!(
                "its answer to `session/new` has no `sessionId`: {result}"
            ))),
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
            permit,
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
                    let answer = match (theirs, work.as_deref_mut()) {
                        ("session/request_permission", Some(work)) => {
                            let outcome = self.permission(&message, work)?;
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
                // A note of the harness. Lira reads the text of the agent,
                // and nothing else yet.
                (Some(theirs), None) => {
                    if let (Some(work), "session/update") = (work.as_deref_mut(), theirs) {
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
                    if let Some(error) = message.get("error") {
                        return Err(Error::Refused {
                            program: self.program.clone(),
                            method: method.to_string(),
                            code: error.get("code").and_then(Json::as_i64).unwrap_or(0),
                            message: error
                                .get("message")
                                .and_then(Json::as_text)
                                .unwrap_or("")
                                .to_string(),
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

    /// Asks `permit`, and gives the outcome for the answer.
    fn permission(&self, message: &Json, work: &mut Work) -> Result<Json, Error> {
        let params = message.get("params");
        let call = params.and_then(|params| params.get("toolCall"));
        let text = |key: &str| {
            call.and_then(|call| call.get(key))
                .and_then(Json::as_text)
                .unwrap_or("")
                .to_string()
        };
        let permission = Permission {
            title: text("title"),
            kind: text("kind"),
            call: call.cloned().unwrap_or(Json::Null),
        };
        // An option for one time is better than an option for always: with
        // "always", the agent does not ask the next time.
        let kinds = if (work.permit)(&permission) {
            ["allow_once", "allow_always"]
        } else {
            ["reject_once", "reject_always"]
        };
        let options = params
            .and_then(|params| params.get("options"))
            .and_then(Json::as_list)
            .unwrap_or(&[]);
        let option = kinds.iter().find_map(|kind| {
            options
                .iter()
                .find(|option| option.get("kind").and_then(Json::as_text) == Some(kind))
                .and_then(|option| option.get("optionId"))
        });
        match option {
            Some(option) => Ok(Json::object([
                ("outcome", Json::text("selected")),
                ("optionId", option.clone()),
            ])),
            None => Err(self.protocol(format!(
                "a permission request has no option of the kind `{}` or `{}`: {message}",
                kinds[0], kinds[1]
            ))),
        }
    }

    fn write(&mut self, method: &str, message: &Json) -> Result<(), Error> {
        let written = match self.input.as_mut() {
            Some(input) => writeln!(input, "{message}").and_then(|()| input.flush()),
            None => Err(ErrorKind::BrokenPipe.into()),
        };
        match written {
            Ok(()) => Ok(()),
            Err(_) => Err(self.ended(method)),
        }
    }

    /// Reads the next message. It skips empty lines.
    fn read(&mut self, method: &str) -> Result<Json, Error> {
        loop {
            let mut line = Vec::new();
            match self.output.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => return Err(self.ended(method)),
                Ok(_) => {}
            }
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
            return match json::read(line) {
                Ok(message @ Json::Object(_)) => Ok(message),
                Ok(_) => Err(self.protocol(format!("a line is not an object: {line}"))),
                Err(error) => Err(self.protocol(format!("a line is not JSON ({error}): {line}"))),
            };
        }
    }

    fn protocol(&self, problem: String) -> Error {
        Error::Protocol {
            program: self.program.clone(),
            problem,
        }
    }

    /// The error for a harness that ended, or that closed its output. Lira
    /// closes the input of the harness, so that it does not stay.
    fn ended(&mut self, method: &str) -> Error {
        self.input = None;
        let status = self.wait().map(|status| status.to_string());
        // The error output ends when the harness ends. A child of the
        // harness can hold it open, so Lira does not wait for long.
        let _ = self.errors_ended.recv_timeout(WAIT);
        let errors = self
            .errors
            .lock()
            .map(|kept| kept.clone())
            .unwrap_or_default();
        Error::Ended {
            program: self.program.clone(),
            method: method.to_string(),
            status,
            errors: String::from_utf8_lossy(&errors).trim().to_string(),
        }
    }

    /// Waits a short time for the harness to end.
    fn wait(&mut self) -> Option<std::process::ExitStatus> {
        let started = Instant::now();
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => return Some(status),
                Ok(None) if started.elapsed() < WAIT => thread::sleep(Duration::from_millis(10)),
                _ => return None,
            }
        }
    }
}

impl Work<'_> {
    /// Keeps the text of an update that is a part of what the agent says.
    fn update(&mut self, message: &Json) {
        let Some(update) = message
            .get("params")
            .and_then(|params| params.get("update"))
        else {
            return;
        };
        if update.get("sessionUpdate").and_then(Json::as_text) != Some("agent_message_chunk") {
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
        let message = update.get("messageId");
        if !self.text.is_empty() && message != self.message.as_ref() {
            self.text.push_str("\n\n");
        }
        self.message = message.cloned();
        self.text.push_str(text);
    }
}

/// When Lira needs the harness no more, it closes the input of the harness.
/// A harness that does not end then is stopped.
impl Drop for Harness {
    fn drop(&mut self) {
        self.input = None;
        if self.wait().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
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
