//! TypeWhisper's API as oso calls it (ADR 0175): the calls, the configuration curl reads them
//! from, what an answer means, and the [`Transport`] that makes a call, through `/usr/bin/curl`
//! or the person's `[dictation] curl`, started through `oso-platform::process`.
//!
//! Must not depend on: `app` or the state of a dictation; this makes one call and says what came
//! back.
//!
//! - curl gets `-q --config -` and nothing else: the address, the token and the body are in the
//!   configuration on its standard input, never in an argument, which other users can read.
//! - The host is `127.0.0.1`, fixed here; only the port comes from the discovery file. No proxy,
//!   no protocol but `http`, no redirect, a time limit for each call and at most [`MAX_ANSWER`]
//!   bytes, with the status written after the body.
//! - curl gets no variable beyond `oso-platform::process`'s base set, and `-q` keeps it from
//!   reading a `.curlrc`.
//! - What TypeWhisper says in an error is cut and cleaned before the echo area shows it.

use std::ffi::OsString;
use std::io::{Read, Write as _};
use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use oso_platform::process::{Executable, Signal, Sink, Source, Spawn, spawn};
use serde_json::Value;

use super::discovery::Discovery;

/// The most bytes of an answer read: a dictation's text is at most a few KiB.
pub const MAX_ANSWER: usize = 1 << 20;

/// The most characters of what TypeWhisper says in an error that the echo area shows.
const MAX_SAID: usize = 120;

/// curl's exit when it could not connect: nothing listens on the port.
const CANNOT_CONNECT: i32 = 7;
/// curl's exit when the call took longer than its limit.
const TIMED_OUT: i32 = 28;
/// curl's exits when the server closed the connection with no answer, or broke it off.
const NO_REPLY: [i32; 3] = [52, 55, 56];
/// curl's exit when the answer was larger than its limit.
const TOO_LARGE: i32 = 63;

/// A dictation's id, as TypeWhisper gave it: a UUID, checked to be one, so that it can go in
/// an address as it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DictationId(String);

impl DictationId {
    /// `text` as an id, if it is a UUID in its usual form.
    pub fn of(text: &str) -> Option<Self> {
        is_uuid(text).then(|| Self(text.to_owned()))
    }
}

/// Whether `text` is a UUID in its usual form: 32 hexadecimal digits in groups of 8, 4, 4, 4
/// and 12, joined by hyphens.
pub fn is_uuid(text: &str) -> bool {
    let groups: Vec<&str> = text.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(group, len)| group.len() == len && group.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// A call to TypeWhisper's API.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Call {
    /// `POST /v1/dictation/start`, in the workflow named, if any (a UUID, checked).
    Start { workflow: Option<String> },
    /// `POST /v1/dictation/stop`.
    Stop,
    /// `GET /v1/dictation/transcription?id=ID`: where the dictation is, and its text once it is
    /// done.
    Transcription(DictationId),
    /// `GET /v1/dictionary/terms`: the person's terms, for a fix (step 2).
    Terms,
    /// `GET /v1/dictionary/corrections`: the person's corrections, for a fix.
    Corrections,
}

impl Call {
    /// Its method, path and body.
    fn request(&self) -> (&'static str, String, Option<String>) {
        match self {
            Self::Start { workflow } => (
                "POST",
                "/v1/dictation/start".to_owned(),
                workflow
                    .as_deref()
                    .filter(|workflow| is_uuid(workflow))
                    .map(|workflow| format!(r#"{{"workflow_id":"{workflow}"}}"#)),
            ),
            Self::Stop => ("POST", "/v1/dictation/stop".to_owned(), None),
            Self::Transcription(DictationId(id)) => {
                ("GET", format!("/v1/dictation/transcription?id={id}"), None)
            }
            Self::Terms => ("GET", "/v1/dictionary/terms".to_owned(), None),
            Self::Corrections => ("GET", "/v1/dictionary/corrections".to_owned(), None),
        }
    }

    /// How long it may take: a start waits for the microphone to be ready.
    pub fn limit(&self) -> Duration {
        match self {
            Self::Start { .. } => Duration::from_secs(10),
            Self::Stop | Self::Transcription(_) | Self::Terms | Self::Corrections => {
                Duration::from_secs(3)
            }
        }
    }
}

/// curl's configuration for `call` to the server `discovery` names, read from its standard
/// input. Every value in it is oso's own or checked: the port is a number, the token's
/// characters need no quoting (`discovery`), and a body holds a UUID alone.
pub fn config(discovery: &Discovery, call: &Call) -> String {
    let (method, path, body) = call.request();
    let mut config = format!(
        "url = \"http://127.0.0.1:{port}{path}\"\n\
         request = \"{method}\"\n\
         noproxy = \"*\"\n\
         proto = \"=http\"\n\
         max-redirs = 0\n\
         max-time = {limit}\n\
         max-filesize = {MAX_ANSWER}\n\
         globoff\n\
         silent\n\
         http1.1\n\
         write-out = \"\\n%{{http_code}}\"\n\
         header = \"Authorization: Bearer {token}\"\n",
        port = discovery.port,
        limit = call.limit().as_secs(),
        token = discovery.token.reveal(),
    );
    if let Some(body) = body {
        config.push_str("header = \"Content-Type: application/json\"\n");
        config.push_str("data-binary = \"");
        config.push_str(&body.replace('\\', "\\\\").replace('"', "\\\""));
        config.push_str("\"\n");
    }
    config
}

/// curl's arguments: it reads its configuration, and only that, from its standard input.
pub fn arguments() -> Vec<OsString> {
    ["-q", "--config", "-"]
        .into_iter()
        .map(OsString::from)
        .collect()
}

/// What came back from a call that reached TypeWhisper: the HTTP status and the body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    pub status: u16,
    pub body: Vec<u8>,
}

/// Why a call brought back no answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// Nothing listens on the port.
    NotRunning,
    /// No answer within the call's limit.
    NoAnswer,
    /// The server closed the connection, or broke it off, with no answer.
    Hung,
    /// The answer was over [`MAX_ANSWER`].
    TooLarge,
    /// curl could not be started or read, or ended with another exit, for this reason.
    Curl(String),
}

/// Reads curl's output: the body, then a line with the status, which `write-out` adds.
///
/// # Errors
///
/// When there is no status at the end, as when curl ended early.
pub fn split_status(output: &[u8]) -> Result<Answer, Failure> {
    let at = output
        .iter()
        .rposition(|&b| b == b'\n')
        .ok_or_else(|| Failure::Curl("curl wrote no status".to_owned()))?;
    let status = std::str::from_utf8(&output[at + 1..])
        .ok()
        .and_then(|status| status.trim().parse::<u16>().ok())
        .filter(|&status| status != 0)
        .ok_or_else(|| Failure::Curl("curl wrote no status".to_owned()))?;
    Ok(Answer {
        status,
        body: output[..at].to_vec(),
    })
}

/// What curl's `exit` code says of a call that wrote no answer.
pub fn failure_of(exit: Option<i32>) -> Failure {
    match exit {
        Some(CANNOT_CONNECT) => Failure::NotRunning,
        Some(TIMED_OUT) => Failure::NoAnswer,
        Some(code) if NO_REPLY.contains(&code) => Failure::Hung,
        Some(TOO_LARGE) => Failure::TooLarge,
        Some(code) => Failure::Curl(format!("curl ended with exit {code}")),
        None => Failure::Curl("curl was stopped by a signal".to_owned()),
    }
}

/// What TypeWhisper's error `body` says, `{"error": {"message": ...}}`, cleaned and cut to
/// [`MAX_SAID`] characters for the echo area, or nothing.
pub fn said(body: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(body).ok()?;
    cleaned(value.get("error")?.get("message")?.as_str()?)
}

/// What TypeWhisper wrote, `text`, as the echo area shows it: its controls as spaces, cut to
/// [`MAX_SAID`] characters and trimmed; nothing if that leaves nothing.
pub fn cleaned(text: &str) -> Option<String> {
    let cleaned: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(MAX_SAID)
        .collect();
    let cleaned = cleaned.trim();
    (!cleaned.is_empty()).then(|| cleaned.to_owned())
}

/// Makes calls to TypeWhisper. The dictation's thread holds one; the tests give it a fake.
pub trait Transport: Send {
    /// Makes `call` to the server `discovery` names, and waits for its answer, within the
    /// call's limit.
    ///
    /// # Errors
    ///
    /// Why no answer came.
    fn call(&mut self, discovery: &Discovery, call: &Call) -> Result<Answer, Failure>;

    /// Starts `call` and does not wait for it, as oso exits: it goes on once oso has gone.
    fn call_and_go(&mut self, discovery: &Discovery, call: &Call);
}

/// The transport through curl.
pub struct Curl {
    executable: Executable,
}

impl Curl {
    /// curl at `path`, which must be an absolute path to an executable file.
    ///
    /// # Errors
    ///
    /// When it is not.
    pub fn at(path: &Path) -> Result<Self, String> {
        Executable::absolute(path)
            .map(|executable| Self { executable })
            .map_err(|_| "curl is not an executable file at [dictation] curl's path".to_owned())
    }

    /// Starts curl with `call`'s configuration on its standard input, which is closed after it,
    /// and its output to `output`.
    fn start(
        &self,
        discovery: &Discovery,
        call: &Call,
        output: Sink,
    ) -> Result<oso_platform::process::Child, Failure> {
        let args = arguments();
        let mut child = spawn(&Spawn {
            executable: &self.executable,
            args: &args,
            cwd: None,
            project: None,
            inherit: &[],
            stdin: Source::Piped,
            stdout: output,
            stderr: Sink::Null,
            own_process_group: false,
            // No terminal to ask on, and it outlives oso when it goes on after an exit.
            own_session: true,
            set: &[],
            terminal: None,
            pass_fd: None,
            args_logged: true,
        })
        .map_err(|e| Failure::Curl(format!("curl did not start: {e}")))?;
        // A few hundred bytes: it fits in the pipe, so this does not wait for curl to read.
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(config(discovery, call).as_bytes());
        }
        Ok(child)
    }
}

impl Transport for Curl {
    fn call(&mut self, discovery: &Discovery, call: &Call) -> Result<Answer, Failure> {
        let mut child = self.start(discovery, call, Sink::Piped)?;
        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Failure::Curl("curl could not be read".to_owned()));
        };
        // curl keeps its own limit; this one, past it, is for a curl that does not.
        let (sent, got) = mpsc::channel();
        let reader = thread::Builder::new()
            .name("oso-dictation-read".into())
            .spawn(move || {
                let mut bytes = Vec::new();
                let read = stdout
                    .take(MAX_ANSWER as u64 + 64)
                    .read_to_end(&mut bytes)
                    .map(|_| bytes);
                let _ = sent.send(read);
            });
        if reader.is_err() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Failure::Curl("curl could not be read".to_owned()));
        }
        let output = match got.recv_timeout(call.limit() + Duration::from_secs(2)) {
            Ok(Ok(output)) => output,
            _ => {
                let _ = child.signal_group(Signal::Kill);
                let _ = child.kill();
                let _ = child.wait();
                return Err(Failure::NoAnswer);
            }
        };
        let status = child
            .wait()
            .map_err(|e| Failure::Curl(format!("curl could not be waited for: {e}")))?;
        match split_status(&output) {
            Ok(answer) if status.success() => Ok(answer),
            _ if status.success() => Err(Failure::Curl("curl wrote no status".to_owned())),
            _ => Err(failure_of(status.code())),
        }
    }

    fn call_and_go(&mut self, discovery: &Discovery, call: &Call) {
        // Its own session lets it finish after oso; nothing waits for it.
        if let Err(e) = self.start(discovery, call, Sink::Null) {
            log::warn!("dictation: the stop at exit did not start: {e:?}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictation::discovery;

    const TOKEN: &str = "abcdefghijklmnopqrstuvwxyz012345";
    const ID: &str = "0F3C2B1A-1234-4ABC-9DEF-0123456789AB";

    fn found() -> Discovery {
        let file = format!(r#"{{"port": 8978, "token": "{TOKEN}", "version": 1}}"#);
        discovery::parse(file.as_bytes()).unwrap()
    }

    #[test]
    fn the_arguments_hold_no_token_no_address_and_read_no_curlrc() {
        let args = arguments();
        assert_eq!(args, ["-q", "--config", "-"], "-q must come first");
        for arg in &args {
            assert!(!arg.to_string_lossy().contains(TOKEN));
        }
    }

    #[test]
    fn a_start_goes_to_the_loopback_with_the_token_and_no_proxy() {
        let config = config(&found(), &Call::Start { workflow: None });
        for line in [
            "url = \"http://127.0.0.1:8978/v1/dictation/start\"",
            "request = \"POST\"",
            "noproxy = \"*\"",
            "proto = \"=http\"",
            "max-redirs = 0",
            "max-time = 10",
            "max-filesize = 1048576",
            "write-out = \"\\n%{http_code}\"",
            &format!("header = \"Authorization: Bearer {TOKEN}\""),
        ] {
            assert!(config.lines().any(|l| l == line), "{line} in\n{config}");
        }
        assert!(!config.contains("data"), "no body without a workflow");
    }

    #[test]
    fn a_workflow_goes_in_the_body_only_as_a_uuid() {
        let call = Call::Start {
            workflow: Some(ID.to_owned()),
        };
        let config = config(&found(), &call);
        assert!(config.contains(&format!(
            "data-binary = \"{{\\\"workflow_id\\\":\\\"{ID}\\\"}}\""
        )));
        assert!(config.contains("Content-Type: application/json"));
        let forged = Call::Start {
            workflow: Some("x\"\nurl = \"http://elsewhere/".to_owned()),
        };
        assert!(!super::config(&found(), &forged).contains("elsewhere"));
    }

    #[test]
    fn a_transcription_names_its_id_and_has_a_short_limit() {
        let id = DictationId::of(ID).unwrap();
        let config = config(&found(), &Call::Transcription(id));
        assert!(config.contains(&format!(
            "url = \"http://127.0.0.1:8978/v1/dictation/transcription?id={ID}\""
        )));
        assert!(config.contains("request = \"GET\""));
        assert!(config.contains("max-time = 3"));
        assert_eq!(DictationId::of("../x"), None);
        assert_eq!(DictationId::of(&ID.replace('-', "")), None);
        assert_eq!(DictationId::of(&format!("{ID}&x=1")), None);
    }

    #[test]
    fn the_dictionarys_reads_are_gets_with_a_short_limit() {
        for (call, path) in [
            (Call::Terms, "/v1/dictionary/terms"),
            (Call::Corrections, "/v1/dictionary/corrections"),
        ] {
            let config = config(&found(), &call);
            assert!(config.contains(&format!("url = \"http://127.0.0.1:8978{path}\"")));
            assert!(config.contains("request = \"GET\""));
            assert!(config.contains("max-time = 3"));
            assert!(!config.contains("data"));
        }
    }

    #[test]
    fn the_status_is_read_after_the_body() {
        assert_eq!(
            split_status(b"{\"id\":1}\n200"),
            Ok(Answer {
                status: 200,
                body: b"{\"id\":1}".to_vec()
            })
        );
        assert_eq!(split_status(b"\n401").unwrap().status, 401);
        assert_eq!(
            split_status(b"line\nmore\n409").unwrap().body,
            b"line\nmore".to_vec()
        );
        assert!(split_status(b"").is_err());
        assert!(split_status(b"body\n000").is_err(), "curl's 000: no answer");
    }

    #[test]
    fn curls_exits_say_why_no_answer_came() {
        assert_eq!(failure_of(Some(7)), Failure::NotRunning);
        assert_eq!(failure_of(Some(28)), Failure::NoAnswer);
        assert_eq!(failure_of(Some(52)), Failure::Hung);
        assert_eq!(failure_of(Some(56)), Failure::Hung);
        assert_eq!(failure_of(Some(63)), Failure::TooLarge);
        assert!(matches!(failure_of(Some(6)), Failure::Curl(_)));
        assert!(matches!(failure_of(None), Failure::Curl(_)));
    }

    #[test]
    fn what_typewhisper_says_is_cleaned_and_cut() {
        let body =
            br#"{"error": {"code": "conflict", "message": "Dictation\u001b[31m is not idle"}}"#;
        assert_eq!(said(body).as_deref(), Some("Dictation [31m is not idle"));
        let long = format!(r#"{{"error": {{"message": "{}"}}}}"#, "x".repeat(500));
        assert_eq!(said(long.as_bytes()).unwrap().chars().count(), MAX_SAID);
        assert_eq!(said(b"{}"), None);
        assert_eq!(said(b"not json"), None);
    }
}
