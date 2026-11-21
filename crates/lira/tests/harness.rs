//! Lira and a harness, through the Agent Client Protocol. A shell script
//! stands in for the harness: it reads what Lira writes, keeps it in a file,
//! and writes the lines that the test gives it.

use lira::harness::{Error, Harness, Permission, Stop};
use lira::json::{self, Json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// What each stand-in starts with. `next` reads one line from Lira and keeps
/// it. `say` writes one line to Lira.
const START: &str = r#"#!/bin/sh
here="$(dirname "$0")"
next() { IFS= read -r line || exit 0; printf '%s\n' "$line" >> "$here/received"; }
say() { printf '%s\n' "$1"; }
"#;

/// The answer to `initialize`.
const HELLO: &str = r#"next; say '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{},"agentInfo":{"name":"stand-in","version":"1"},"authMethods":[]}}'
"#;

/// The answer to `session/new`.
const CONVERSATION: &str = r#"next; say '{"jsonrpc":"2.0","id":2,"result":{"sessionId":"c-1"}}'
"#;

struct StandIn {
    folder: PathBuf,
}

impl StandIn {
    /// A stand-in in a new folder of its own, with this script after `START`.
    fn new(name: &str, script: &str) -> StandIn {
        let folder = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("harness")
            .join(name);
        if folder.exists() {
            fs::remove_dir_all(&folder).unwrap();
        }
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("harness.sh"), format!("{START}{script}")).unwrap();
        StandIn { folder }
    }

    fn command(&self) -> Command {
        let mut command = Command::new("sh");
        command.arg(self.folder.join("harness.sh"));
        command
    }

    fn start(&self) -> Harness {
        Harness::start(self.command()).unwrap()
    }

    /// The message that Lira wrote on this line. The first line is 1.
    fn received(&self, line: usize) -> Json {
        let text = fs::read_to_string(self.folder.join("received")).unwrap();
        let line = text
            .lines()
            .nth(line - 1)
            .unwrap_or_else(|| panic!("Lira wrote no line {line}:\n{text}"));
        json::read(line).unwrap()
    }
}

fn read(text: &str) -> Json {
    json::read(text).unwrap()
}

fn yes(_: &Permission) -> bool {
    true
}

fn no(_: &Permission) -> bool {
    false
}

#[test]
fn lira_starts_the_harness_and_they_agree_on_the_version() {
    let stand_in = StandIn::new("version", HELLO);
    let _harness = stand_in.start();
    let expected = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":1,"clientCapabilities":{{"fs":{{"readTextFile":false,"writeTextFile":false}},"terminal":false}},"clientInfo":{{"name":"lira","version":"{}"}}}}}}"#,
        env!("CARGO_PKG_VERSION")
    );
    assert_eq!(stand_in.received(1), read(&expected));
}

#[test]
fn a_harness_with_another_version_of_the_protocol_is_an_error() {
    let stand_in = StandIn::new(
        "other-version",
        r#"next; say '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":2}}'
"#,
    );
    let error = Harness::start(stand_in.command()).err().unwrap();
    assert!(matches!(error, Error::Protocol { .. }), "{error:?}");
    let text = error.to_string();
    assert!(text.contains("version 2"), "{text}");
    assert!(text.contains("version 1"), "{text}");
}

#[test]
fn lira_starts_a_conversation_in_a_folder() {
    let stand_in = StandIn::new("conversation", &format!("{HELLO}{CONVERSATION}"));
    let mut harness = stand_in.start();
    let extra = read(r#"{"claudeCode":{"options":{"model":"small"}}}"#);
    let conversation = harness.converse(&stand_in.folder, Some(extra)).unwrap();
    assert_eq!(conversation, "c-1");
    let folder = Json::text(stand_in.folder.to_str().unwrap());
    let expected = format!(
        r#"{{"jsonrpc":"2.0","id":2,"method":"session/new","params":{{"cwd":{folder},"mcpServers":[],"_meta":{{"claudeCode":{{"options":{{"model":"small"}}}}}}}}}}"#
    );
    assert_eq!(stand_in.received(2), read(&expected));
}

#[test]
fn a_folder_that_is_not_absolute_is_made_absolute() {
    let stand_in = StandIn::new("relative", &format!("{HELLO}{CONVERSATION}"));
    let mut harness = stand_in.start();
    harness.converse(Path::new("some/folder"), None).unwrap();
    let message = stand_in.received(2);
    let folder = message.get("params").unwrap().get("cwd").unwrap();
    let folder = Path::new(folder.as_text().unwrap());
    assert!(folder.is_absolute(), "{folder:?}");
    assert!(folder.ends_with("some/folder"), "{folder:?}");
    // With nothing extra, the message has no `_meta`.
    assert_eq!(message.get("params").unwrap().get("_meta"), None);
}

#[test]
fn lira_sends_a_message_and_reads_the_reply() {
    let script = r#"next
say '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"c-1","update":{"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":"a thought"}}}}'
say '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"c-1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"I make "},"messageId":"m-1"}}}'
say '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"c-1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"the file."},"messageId":"m-1"}}}'
say ''
say '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"c-1","update":{"sessionUpdate":"tool_call","toolCallId":"t-1","title":"Write hello.txt","kind":"edit","status":"pending"}}}'
say '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"c-1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"image","data":"AAAA","mimeType":"image/png"},"messageId":"m-2"}}}'
say '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"c-1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Done: \"hi\" é"},"messageId":"m-2"}}}'
say '{"jsonrpc":"2.0","method":"a/new_kind_of_note","params":{}}'
say '{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn","usage":{"totalTokens":12}}}'
"#;
    let stand_in = StandIn::new("reply", &format!("{HELLO}{CONVERSATION}{script}"));
    let mut harness = stand_in.start();
    let conversation = harness.converse(&stand_in.folder, None).unwrap();
    let message = "Make \"hello.txt\".\nIt holds the word hi. \\ é";
    let reply = harness.send(&conversation, message, &mut yes).unwrap();
    assert_eq!(reply.text, "I make the file.\n\nDone: \"hi\" é");
    assert_eq!(reply.stop, Stop::End);
    let expected = Json::object([
        ("jsonrpc", Json::text("2.0")),
        ("id", Json::number(3)),
        ("method", Json::text("session/prompt")),
        (
            "params",
            Json::object([
                ("sessionId", Json::text("c-1")),
                (
                    "prompt",
                    Json::List(vec![Json::object([
                        ("type", Json::text("text")),
                        ("text", Json::text(message)),
                    ])]),
                ),
            ]),
        ),
    ]);
    assert_eq!(stand_in.received(3), expected);
}

#[test]
fn the_reply_says_why_the_agent_stopped() {
    let reasons = [
        ("end_turn", Stop::End),
        ("max_tokens", Stop::MaxTokens),
        ("max_turn_requests", Stop::MaxRequests),
        ("refusal", Stop::Refusal),
        ("cancelled", Stop::Cancelled),
        ("a_new_reason", Stop::Other("a_new_reason".to_string())),
    ];
    for (reason, stop) in reasons {
        let script = format!(
            "next; say '{{\"jsonrpc\":\"2.0\",\"id\":3,\"result\":{{\"stopReason\":\"{reason}\"}}}}'\n"
        );
        let stand_in = StandIn::new(
            &format!("stop-{reason}"),
            &format!("{HELLO}{CONVERSATION}{script}"),
        );
        let mut harness = stand_in.start();
        let conversation = harness.converse(&stand_in.folder, None).unwrap();
        let reply = harness.send(&conversation, "Go.", &mut yes).unwrap();
        assert_eq!(reply.stop, stop);
        assert_eq!(reply.text, "");
    }
}

/// A permission request with the four kinds of option, then the end of the
/// reply. The stand-in keeps Lira's answer as line 4.
fn permission_script(id: &str) -> String {
    format!(
        r#"next
say '{{"jsonrpc":"2.0","id":{id},"method":"session/request_permission","params":{{"sessionId":"c-1","toolCall":{{"toolCallId":"t-1","title":"Write a.txt","kind":"edit","status":"pending","rawInput":{{"file_path":"/w/a.txt"}}}},"options":[{{"optionId":"always","name":"Yes, always","kind":"allow_always"}},{{"optionId":"once","name":"Yes","kind":"allow_once"}},{{"optionId":"never","name":"No, never","kind":"reject_always"}},{{"optionId":"not-now","name":"No","kind":"reject_once"}}]}}}}'
next
say '{{"jsonrpc":"2.0","id":3,"result":{{"stopReason":"end_turn"}}}}'
"#
    )
}

#[test]
fn lira_answers_a_permission_request_with_yes() {
    let script = permission_script("7");
    let stand_in = StandIn::new("yes", &format!("{HELLO}{CONVERSATION}{script}"));
    let mut harness = stand_in.start();
    let conversation = harness.converse(&stand_in.folder, None).unwrap();
    let mut asked = Vec::new();
    let mut permit = |permission: &Permission| {
        asked.push((
            permission.title.clone(),
            permission.kind.clone(),
            permission.call.get("toolCallId").cloned(),
        ));
        true
    };
    let reply = harness.send(&conversation, "Go.", &mut permit).unwrap();
    assert_eq!(reply.stop, Stop::End);
    assert_eq!(
        asked,
        [(
            "Write a.txt".to_string(),
            "edit".to_string(),
            Some(Json::text("t-1"))
        )]
    );
    assert_eq!(
        stand_in.received(4),
        read(
            r#"{"jsonrpc":"2.0","id":7,"result":{"outcome":{"outcome":"selected","optionId":"once"}}}"#
        )
    );
}

#[test]
fn lira_answers_a_permission_request_with_no() {
    // The protocol lets the number of a request be a text.
    let script = permission_script("\"p-1\"");
    let stand_in = StandIn::new("no", &format!("{HELLO}{CONVERSATION}{script}"));
    let mut harness = stand_in.start();
    let conversation = harness.converse(&stand_in.folder, None).unwrap();
    let reply = harness.send(&conversation, "Go.", &mut no).unwrap();
    assert_eq!(reply.stop, Stop::End);
    assert_eq!(
        stand_in.received(4),
        read(
            r#"{"jsonrpc":"2.0","id":"p-1","result":{"outcome":{"outcome":"selected","optionId":"not-now"}}}"#
        )
    );
}

#[test]
fn with_no_option_for_one_time_lira_takes_the_option_for_always() {
    let script = r#"next
say '{"jsonrpc":"2.0","id":0,"method":"session/request_permission","params":{"sessionId":"c-1","toolCall":{"toolCallId":"t-1"},"options":[{"optionId":"always","name":"Yes, always","kind":"allow_always"},{"optionId":"never","name":"No, never","kind":"reject_always"}]}}'
next
say '{"jsonrpc":"2.0","id":1,"method":"session/request_permission","params":{"sessionId":"c-1","toolCall":{"toolCallId":"t-2"},"options":[{"optionId":"always","name":"Yes, always","kind":"allow_always"},{"optionId":"never","name":"No, never","kind":"reject_always"}]}}'
next
say '{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}'
"#;
    let stand_in = StandIn::new("always", &format!("{HELLO}{CONVERSATION}{script}"));
    let mut harness = stand_in.start();
    let conversation = harness.converse(&stand_in.folder, None).unwrap();
    let mut answers = [true, false].into_iter();
    let mut permit = |permission: &Permission| {
        // A request with no title and no kind still reaches the code.
        assert_eq!(permission.title, "");
        assert_eq!(permission.kind, "");
        answers.next().unwrap()
    };
    harness.send(&conversation, "Go.", &mut permit).unwrap();
    assert_eq!(
        stand_in.received(4),
        read(
            r#"{"jsonrpc":"2.0","id":0,"result":{"outcome":{"outcome":"selected","optionId":"always"}}}"#
        )
    );
    assert_eq!(
        stand_in.received(5),
        read(
            r#"{"jsonrpc":"2.0","id":1,"result":{"outcome":{"outcome":"selected","optionId":"never"}}}"#
        )
    );
}

#[test]
fn with_no_option_for_its_answer_lira_answers_that_it_chose_none() {
    // The protocol does not say that a request has an option of each kind.
    let script = r#"next
say '{"jsonrpc":"2.0","id":0,"method":"session/request_permission","params":{"sessionId":"c-1","toolCall":{"toolCallId":"t-1"},"options":[{"optionId":"once","name":"Yes","kind":"allow_once"}]}}'
next
say '{"jsonrpc":"2.0","id":1,"method":"session/request_permission","params":{"sessionId":"c-1","toolCall":{"toolCallId":"t-2"},"options":[{"optionId":"never","name":"No","kind":"reject_always"}]}}'
next
say '{"jsonrpc":"2.0","id":2,"method":"session/request_permission","params":{"sessionId":"c-1","toolCall":{"toolCallId":"t-3"}}}'
next
say '{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}'
"#;
    let stand_in = StandIn::new("no-option", &format!("{HELLO}{CONVERSATION}{script}"));
    let mut harness = stand_in.start();
    let conversation = harness.converse(&stand_in.folder, None).unwrap();
    let mut answers = [false, true, true].into_iter();
    let mut permit = |_: &Permission| answers.next().unwrap();
    let reply = harness.send(&conversation, "Go.", &mut permit).unwrap();
    assert_eq!(reply.stop, Stop::End);
    for (line, id) in [(4, 0), (5, 1), (6, 2)] {
        let expected = format!(
            r#"{{"jsonrpc":"2.0","id":{id},"result":{{"outcome":{{"outcome":"cancelled"}}}}}}"#
        );
        assert_eq!(stand_in.received(line), read(&expected));
    }
}

#[test]
fn a_request_that_lira_does_not_know_gets_an_error_as_its_answer() {
    let script = r#"next
say '{"jsonrpc":"2.0","id":9,"method":"fs/read_text_file","params":{"sessionId":"c-1","path":"/w/a.txt"}}'
next
say '{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}'
"#;
    let stand_in = StandIn::new("unknown", &format!("{HELLO}{CONVERSATION}{script}"));
    let mut harness = stand_in.start();
    let conversation = harness.converse(&stand_in.folder, None).unwrap();
    let reply = harness.send(&conversation, "Go.", &mut yes).unwrap();
    assert_eq!(reply.stop, Stop::End);
    let answer = stand_in.received(4);
    assert_eq!(answer.get("id"), Some(&Json::number(9)));
    let error = answer.get("error").unwrap();
    assert_eq!(error.get("code"), Some(&Json::number(-32601)));
    let message = error.get("message").unwrap().as_text().unwrap();
    assert!(message.contains("fs/read_text_file"), "{message}");
}

#[test]
fn a_harness_that_is_not_on_the_host_is_an_error() {
    let error = Harness::start(Command::new("lira-has-no-such-harness"))
        .err()
        .unwrap();
    assert!(matches!(error, Error::Start { .. }), "{error:?}");
    assert_eq!(
        error.to_string(),
        "Lira can't find the harness `lira-has-no-such-harness` on this host"
    );

    // A folder that is not there is not a harness that is not there.
    let mut command = Command::new("sh");
    command.current_dir("/lira-has-no-such-folder");
    let error = Harness::start(command).err().unwrap();
    assert!(matches!(error, Error::Start { .. }), "{error:?}");
    let text = error.to_string();
    assert!(
        text.contains("the folder `/lira-has-no-such-folder`"),
        "{text}"
    );
    assert!(!text.contains("can't find the harness"), "{text}");
}

#[test]
fn a_harness_that_ends_before_the_reply_is_an_error() {
    let script = r#"next
say '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"c-1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"I start"}}}}'
echo "the stand-in has no more memory" >&2
exit 3
"#;
    let stand_in = StandIn::new("ends", &format!("{HELLO}{CONVERSATION}{script}"));
    let mut harness = stand_in.start();
    let conversation = harness.converse(&stand_in.folder, None).unwrap();
    let error = harness.send(&conversation, "Go.", &mut yes).err().unwrap();
    assert!(matches!(error, Error::Ended { .. }), "{error:?}");
    let text = error.to_string();
    assert!(text.contains("`sh`"), "{text}");
    assert!(text.contains("session/prompt"), "{text}");
    assert!(text.contains("exit status: 3"), "{text}");
    assert!(text.contains("the stand-in has no more memory"), "{text}");

    // The harness is gone, so the next message is the same kind of error.
    let error = harness.send(&conversation, "Go.", &mut yes).err().unwrap();
    assert!(matches!(error, Error::Ended { .. }), "{error:?}");
}

#[test]
fn an_error_from_the_harness_is_an_error_with_its_parts() {
    let script = r#"next
say '{"jsonrpc":"2.0","id":3,"error":{"code":-32603,"message":"Internal error","data":{"errorKind":"rate_limit"}}}'
next
say '{"jsonrpc":"2.0","id":4,"result":{"stopReason":"end_turn"}}'
"#;
    let stand_in = StandIn::new("refused", &format!("{HELLO}{CONVERSATION}{script}"));
    let mut harness = stand_in.start();
    let conversation = harness.converse(&stand_in.folder, None).unwrap();
    let error = harness.send(&conversation, "Go.", &mut yes).err().unwrap();
    match &error {
        Error::Refused {
            method,
            code,
            message,
            data,
            ..
        } => {
            assert_eq!(method, "session/prompt");
            assert_eq!(*code, -32603);
            assert_eq!(message, "Internal error");
            assert_eq!(*data, Some(read(r#"{"errorKind":"rate_limit"}"#)));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        error.to_string(),
        "the harness `sh` answered `session/prompt` with error -32603: Internal error\n{\"errorKind\":\"rate_limit\"}"
    );

    // The harness still runs, so the next message gets its reply.
    let reply = harness.send(&conversation, "Go.", &mut yes).unwrap();
    assert_eq!(reply.stop, Stop::End);
}

#[test]
fn a_line_that_is_not_a_message_of_the_protocol_is_an_error() {
    let lines = [
        ("text", "Welcome to the stand-in!"),
        ("list", "[1,2]"),
        ("empty", "{}"),
        ("other-id", r#"{"jsonrpc":"2.0","id":8,"result":{}}"#),
        ("no-result", r#"{"jsonrpc":"2.0","id":3}"#),
        ("no-reason", r#"{"jsonrpc":"2.0","id":3,"result":{}}"#),
        (
            "old-rpc",
            r#"{"jsonrpc":"1.0","id":3,"result":{"stopReason":"end_turn"}}"#,
        ),
        (
            "null-rpc",
            r#"{"jsonrpc":null,"id":3,"result":{"stopReason":"end_turn"}}"#,
        ),
        ("no-rpc", r#"{"id":3,"result":{"stopReason":"end_turn"}}"#),
    ];
    for (name, line) in lines {
        let script = format!("next; say '{line}'\nnext\n");
        let stand_in = StandIn::new(
            &format!("line-{name}"),
            &format!("{HELLO}{CONVERSATION}{script}"),
        );
        let mut harness = stand_in.start();
        let conversation = harness.converse(&stand_in.folder, None).unwrap();
        let error = harness.send(&conversation, "Go.", &mut yes).err().unwrap();
        assert!(matches!(error, Error::Protocol { .. }), "{name}: {error:?}");
        assert!(error.to_string().contains("`sh`"), "{error}");
    }
}

#[test]
fn lira_stops_the_harness_when_it_needs_it_no_more() {
    // A harness that ends when its input ends.
    let stand_in = StandIn::new(
        "stops",
        &format!("trap 'echo yes > \"$here/ended\"' EXIT\n{HELLO}next\n"),
    );
    let harness = stand_in.start();
    drop(harness);
    assert_eq!(
        fs::read_to_string(stand_in.folder.join("ended")).unwrap(),
        "yes\n"
    );

    // A harness that does not end when its input ends.
    let stand_in = StandIn::new(
        "stops-not",
        &format!("{HELLO}echo $$ > \"$here/pid\"\nexec sleep 60\n"),
    );
    let harness = stand_in.start();
    let pid = stand_in.folder.join("pid");
    let waited = Instant::now();
    while !pid.exists() || fs::read_to_string(&pid).unwrap().is_empty() {
        assert!(waited.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(10));
    }
    let pid = fs::read_to_string(&pid).unwrap();
    let started = Instant::now();
    drop(harness);
    assert!(started.elapsed() < Duration::from_secs(30));
    let alive = Command::new("kill")
        .args(["-0", pid.trim()])
        .output()
        .unwrap();
    assert!(!alive.status.success(), "the harness {pid} still runs");
}

/// Runs `work` on a thread of its own, and fails the test when it takes
/// longer than a minute. A test of a hang must not hang.
fn in_time<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    let (done, result) = mpsc::channel();
    thread::spawn(move || {
        let _ = done.send(work());
    });
    result
        .recv_timeout(Duration::from_secs(60))
        .expect("Lira did not come back in a minute")
}

/// If a process with this number is there, and is not a process that ended
/// and waits for its parent.
fn runs(pid: &str) -> bool {
    let state = Command::new("ps")
        .args(["-o", "stat=", "-p", pid.trim()])
        .output()
        .unwrap();
    let state = String::from_utf8_lossy(&state.stdout);
    !state.trim().is_empty() && !state.trim().starts_with('Z')
}

/// Waits a short time for a process to be gone.
fn is_gone(pid: &str) -> bool {
    let started = Instant::now();
    while runs(pid) {
        if started.elapsed() > Duration::from_secs(5) {
            return false;
        }
        thread::sleep(Duration::from_millis(20));
    }
    true
}

/// The text of a file that a stand-in writes, when it is complete.
fn written(path: &Path) -> String {
    let started = Instant::now();
    loop {
        let text = fs::read_to_string(path).unwrap_or_default();
        if text.ends_with('\n') {
            return text.trim().to_string();
        }
        assert!(started.elapsed() < Duration::from_secs(10), "{path:?}");
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn lira_and_the_harness_can_write_much_at_the_same_time() {
    // The stand-in writes a long line before it reads the message, and the
    // message is long too. Each is more than a pipe holds.
    let script = r#"printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"c-1","update":{"sessionUpdate":"available_commands_update","note":"'
head -c 400000 /dev/zero | tr '\0' a
printf '"}}}\n'
IFS= read -r line || exit 0
printf '%s' "$line" | wc -c > "$here/length"
say '{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}'
"#;
    let stand_in = StandIn::new("much", &format!("{HELLO}{CONVERSATION}{script}"));
    let folder = stand_in.folder.clone();
    let reply = in_time(move || {
        let mut harness = stand_in.start();
        let conversation = harness.converse(&stand_in.folder, None).unwrap();
        let message = "b".repeat(400_000);
        harness.send(&conversation, &message, &mut yes)
    })
    .unwrap();
    assert_eq!(reply.stop, Stop::End);
    assert_eq!(reply.text, "");
    let length: usize = written(&folder.join("length")).parse().unwrap();
    assert!(length > 400_000, "{length}");
}

#[test]
fn a_harness_that_ends_while_its_child_holds_the_output_is_an_error() {
    // The child of the stand-in has the output of the stand-in, so the
    // output does not end when the stand-in does.
    let script = r#"next
sleep 300 &
echo $! > "$here/child"
echo "the stand-in gives up" >&2
exit 7
"#;
    let stand_in = StandIn::new("child-holds", &format!("{HELLO}{CONVERSATION}{script}"));
    let folder = stand_in.folder.clone();
    let started = Instant::now();
    let error = in_time(move || {
        let mut harness = stand_in.start();
        let conversation = harness.converse(&stand_in.folder, None).unwrap();
        harness.send(&conversation, "Go.", &mut yes).err().unwrap()
    });
    assert!(started.elapsed() < Duration::from_secs(30));
    assert!(matches!(error, Error::Ended { .. }), "{error:?}");
    let text = error.to_string();
    assert!(text.contains("exit status: 7"), "{text}");
    assert!(text.contains("the stand-in gives up"), "{text}");
    // Lira stopped the child too.
    assert!(is_gone(&written(&folder.join("child"))));
}

#[test]
fn a_harness_that_closes_its_output_and_stays_is_stopped() {
    let script = r#"next
echo $$ > "$here/pid"
exec sleep 300 >&-
"#;
    let stand_in = StandIn::new("closes-output", &format!("{HELLO}{CONVERSATION}{script}"));
    let folder = stand_in.folder.clone();
    let (error, again) = in_time(move || {
        let mut harness = stand_in.start();
        let conversation = harness.converse(&stand_in.folder, None).unwrap();
        let error = harness.send(&conversation, "Go.", &mut yes).err().unwrap();
        let again = harness.send(&conversation, "Go.", &mut yes).err().unwrap();
        // The harness is gone before Lira needs it no more.
        assert!(is_gone(&written(&stand_in.folder.join("pid"))));
        (error, again)
    });
    assert!(matches!(error, Error::Ended { .. }), "{error:?}");
    let text = error.to_string();
    assert!(text.contains("closed its output"), "{text}");
    assert!(text.contains("Lira stopped it"), "{text}");
    assert!(!text.contains("exit status"), "{text}");
    assert!(matches!(again, Error::Ended { .. }), "{again:?}");
    assert!(is_gone(&written(&folder.join("pid"))));
}

#[test]
fn lira_stops_the_children_of_the_harness_too() {
    // The stand-in ends when its input ends, and its child does not.
    let script = r#"sleep 300 &
echo $! > "$here/child"
next
"#;
    let stand_in = StandIn::new("children", &format!("{HELLO}{script}"));
    let harness = stand_in.start();
    let child = written(&stand_in.folder.join("child"));
    assert!(runs(&child));
    drop(harness);
    assert!(is_gone(&child));
}

#[test]
fn parts_of_one_message_with_no_name_are_one_text() {
    let script = r#"next
say '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"c-1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hel"}}}}'
say '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"c-1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"lo"},"messageId":null}}}'
say '{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}'
"#;
    let stand_in = StandIn::new("no-name", &format!("{HELLO}{CONVERSATION}{script}"));
    let mut harness = stand_in.start();
    let conversation = harness.converse(&stand_in.folder, None).unwrap();
    let reply = harness.send(&conversation, "Go.", &mut yes).unwrap();
    assert_eq!(reply.text, "hello");
}
