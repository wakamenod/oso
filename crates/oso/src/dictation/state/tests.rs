//! The dictation's thread over a fake transport: its steps, each failure's words, the stop
//! from elsewhere, the limit, and the stop that goes as oso exits.
//!
//! Must not depend on: curl or a TypeWhisper; the fake answers each call.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex};

use super::*;

const ID: &str = "0F3C2B1A-1234-4ABC-9DEF-0123456789AB";

/// Short waits, so that a test runs in a moment.
const FAST: Timing = Timing {
    recording_poll: Duration::from_millis(10),
    text_poll: Duration::from_millis(5),
    text_limit: Duration::from_millis(300),
    settle: Duration::from_millis(20),
};

/// What a fake call gives.
type Reply = Result<Answer, Failure>;

/// A transport that answers each kind of call from its own list, the last answer again once
/// the list runs out, and keeps what it was asked.
#[derive(Clone, Default)]
struct Fake {
    starts: Arc<Mutex<VecDeque<Reply>>>,
    stops: Arc<Mutex<VecDeque<Reply>>>,
    polls: Arc<Mutex<VecDeque<Reply>>>,
    calls: Arc<Mutex<Vec<String>>>,
}

fn ok(status: u16, body: &str) -> Reply {
    Ok(Answer {
        status,
        body: body.as_bytes().to_vec(),
    })
}

fn started() -> Reply {
    ok(200, &format!(r#"{{"id": "{ID}", "status": "recording"}}"#))
}

fn at(status: &str) -> Reply {
    ok(200, &format!(r#"{{"id": "{ID}", "status": "{status}"}}"#))
}

fn completed(text: &str) -> Reply {
    let body = serde_json::json!({
        "id": ID,
        "status": "completed",
        "transcription": { "text": text, "raw_text": text },
    });
    ok(200, &body.to_string())
}

/// What [`completed`]'s answer says was heard.
fn heard(text: &str) -> Heard {
    Heard {
        text: text.to_owned(),
        raw: Some(text.to_owned()),
    }
}

impl Fake {
    fn with(starts: Vec<Reply>, stops: Vec<Reply>, polls: Vec<Reply>) -> Self {
        Self {
            starts: Arc::new(Mutex::new(starts.into())),
            stops: Arc::new(Mutex::new(stops.into())),
            polls: Arc::new(Mutex::new(polls.into())),
            calls: Arc::default(),
        }
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

fn next(list: &Mutex<VecDeque<Reply>>) -> Reply {
    let mut list = list.lock().unwrap();
    if list.len() > 1 {
        list.pop_front().unwrap()
    } else {
        list.front().cloned().unwrap_or(Err(Failure::NotRunning))
    }
}

impl Transport for Fake {
    fn call(&mut self, discovery: &Discovery, call: &Call) -> Result<Answer, Failure> {
        assert_eq!(discovery.port, 8978);
        let (name, list) = match call {
            Call::Start { .. } => ("start", &self.starts),
            Call::Stop => ("stop", &self.stops),
            Call::Transcription(_) => ("poll", &self.polls),
            _ => panic!("a dictation reads no dictionary"),
        };
        self.calls.lock().unwrap().push(name.to_owned());
        next(list)
    }

    fn call_and_go(&mut self, _: &Discovery, call: &Call) {
        assert_eq!(call, &Call::Stop);
        self.calls.lock().unwrap().push("stop and go".to_owned());
    }
}

/// A discovery file in a directory of the test's own.
fn discovery_file(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("oso-dictation-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("api-discovery.json");
    std::fs::write(
        &path,
        r#"{"port": 8978, "token": "abcdefghijklmnopqrstuvwxyz012345", "version": 1}"#,
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    path
}

fn settings(discovery: &Path) -> Settings {
    Settings {
        discovery: discovery.to_owned(),
        workflow: None,
        max_recording: Duration::from_secs(60),
    }
}

/// Starts a dictation over `fake`, and gives its handle and its events as they come.
fn begin(settings: Settings, fake: &Fake) -> (Handle, Receiver<Event>) {
    let (tx, events) = mpsc::channel();
    let fake = fake.clone();
    let make: MakeTransport = Box::new(move || Ok(Box::new(fake) as Box<dyn Transport>));
    let handle = start(settings, make, FAST, move |event| {
        let _ = tx.send(event);
    })
    .unwrap();
    (handle, events)
}

fn event(events: &Receiver<Event>) -> Event {
    events.recv_timeout(Duration::from_secs(5)).unwrap()
}

#[test]
fn a_dictation_records_stops_and_gives_its_text() {
    let file = discovery_file("text");
    let fake = Fake::with(
        vec![started()],
        vec![ok(200, "{}")],
        vec![at("recording"), at("processing"), completed("こんにちは")],
    );
    let (handle, events) = begin(settings(&file), &fake);
    assert_eq!(event(&events), Event::Recording);
    handle.stop();
    assert_eq!(event(&events), Event::Waiting(Stopped::Asked));
    assert_eq!(event(&events), Event::Text(heard("こんにちは")));
    assert_eq!(event(&events), Event::Settled);
    let calls = fake.calls();
    assert_eq!(calls.first().map(String::as_str), Some("start"));
    assert!(calls.contains(&"stop".to_owned()));
    assert_eq!(calls.last().map(String::as_str), Some("poll"));
}

#[test]
fn no_discovery_file_says_typewhisper_is_not_running_and_calls_nothing() {
    let file = discovery_file("missing");
    std::fs::remove_file(&file).unwrap();
    let fake = Fake::default();
    let (_handle, events) = begin(settings(&file), &fake);
    assert_eq!(event(&events), Event::Failed(NOT_RUNNING.to_owned()));
    assert!(fake.calls().is_empty(), "no fallback to a usual port");
}

#[test]
fn each_refusal_of_the_start_says_why_and_nothing_is_tried_again() {
    let file = discovery_file("refused");
    let cases = [
        (Err(Failure::NotRunning), NOT_RUNNING.to_owned()),
        (
            Err(Failure::NoAnswer),
            "TypeWhisper did not answer in 10 s".to_owned(),
        ),
        (
            ok(
                401,
                r#"{"error": {"code": "unauthorized", "message": "Missing"}}"#,
            ),
            "TypeWhisper refused oso's token: is its discovery file current?".to_owned(),
        ),
        (
            ok(
                409,
                r#"{"error": {"code": "conflict", "message": "Dictation is not idle"}}"#,
            ),
            "TypeWhisper can't dictate now: Dictation is not idle".to_owned(),
        ),
        (
            Err(Failure::TooLarge),
            "TypeWhisper's answer was too large".to_owned(),
        ),
        (
            ok(200, r#"{"id": "not-a-uuid"}"#),
            "TypeWhisper's answer named no dictation".to_owned(),
        ),
    ];
    for (reply, said) in cases {
        let fake = Fake::with(vec![reply], vec![], vec![]);
        let (_handle, events) = begin(settings(&file), &fake);
        assert_eq!(event(&events), Event::Failed(said));
        assert_eq!(fake.calls(), ["start"], "never again");
    }
}

#[test]
fn a_stop_from_typewhispers_own_hotkey_goes_on_to_the_text() {
    let file = discovery_file("elsewhere");
    let fake = Fake::with(
        vec![started()],
        vec![],
        vec![at("recording"), at("processing"), completed("done")],
    );
    let (_handle, events) = begin(settings(&file), &fake);
    assert_eq!(event(&events), Event::Recording);
    assert_eq!(event(&events), Event::Waiting(Stopped::Elsewhere));
    assert_eq!(event(&events), Event::Text(heard("done")));
    assert!(!fake.calls().contains(&"stop".to_owned()));
}

#[test]
fn recording_past_the_limit_stops_it() {
    let file = discovery_file("limit");
    let fake = Fake::with(vec![started()], vec![ok(200, "{}")], vec![at("recording")]);
    let mut settings = settings(&file);
    settings.max_recording = Duration::from_millis(40);
    let (_handle, events) = begin(settings, &fake);
    assert_eq!(event(&events), Event::Recording);
    assert_eq!(event(&events), Event::Waiting(Stopped::Limit));
    // It goes on recording, as far as the fake says, until the text's limit.
    assert_eq!(
        event(&events),
        Event::Failed("No text came from TypeWhisper in 0 s".to_owned())
    );
}

#[test]
fn a_stop_typewhisper_already_made_is_no_failure() {
    let file = discovery_file("409");
    let fake = Fake::with(
        vec![started()],
        vec![ok(409, r#"{"error": {"message": "Not recording"}}"#)],
        vec![at("recording"), completed("late")],
    );
    let (handle, events) = begin(settings(&file), &fake);
    assert_eq!(event(&events), Event::Recording);
    handle.stop();
    assert_eq!(event(&events), Event::Waiting(Stopped::Asked));
    assert_eq!(event(&events), Event::Text(heard("late")));
}

#[test]
fn typewhisper_that_dies_or_fails_or_forgets_ends_the_dictation() {
    let file = discovery_file("dies");
    for (reply, said) in [
        (Err(Failure::NotRunning), "TypeWhisper stopped answering"),
        (Err(Failure::Hung), "TypeWhisper stopped answering"),
        (ok(404, "{}"), "TypeWhisper no longer has the dictation"),
        (
            ok(
                200,
                &format!(r#"{{"id": "{ID}", "status": "failed", "error": "no\u0007mic"}}"#),
            ),
            "TypeWhisper failed: no mic",
        ),
        (ok(200, "[]"), "TypeWhisper's answer was not one oso reads"),
    ] {
        let fake = Fake::with(vec![started()], vec![ok(200, "{}")], vec![reply]);
        let (handle, events) = begin(settings(&file), &fake);
        assert_eq!(event(&events), Event::Recording);
        handle.stop();
        let mut seen = event(&events);
        if seen == Event::Waiting(Stopped::Asked) {
            seen = event(&events);
        }
        assert_eq!(seen, Event::Failed(said.to_owned()));
    }
}

#[test]
fn as_oso_exits_a_recording_dictation_starts_a_stop_and_does_not_wait() {
    let file = discovery_file("exit");
    let fake = Fake::with(vec![started()], vec![], vec![at("recording")]);
    let (handle, events) = begin(settings(&file), &fake);
    assert_eq!(event(&events), Event::Recording);
    let began = Instant::now();
    handle.leave(Duration::from_secs(2));
    assert!(began.elapsed() < Duration::from_secs(1));
    assert_eq!(fake.calls().last().map(String::as_str), Some("stop and go"));
    assert!(
        events.try_recv().is_err(),
        "nothing said after the host went"
    );
}

#[test]
fn a_dictation_that_stopped_says_nothing_after_the_host_went() {
    let file = discovery_file("left");
    let fake = Fake::with(vec![started()], vec![ok(200, "{}")], vec![at("processing")]);
    let (handle, events) = begin(settings(&file), &fake);
    assert_eq!(event(&events), Event::Recording);
    handle.stop();
    assert_eq!(event(&events), Event::Waiting(Stopped::Asked));
    handle.leave(Duration::from_secs(2));
    assert!(!fake.calls().contains(&"stop and go".to_owned()));
    std::thread::sleep(Duration::from_millis(50));
    assert!(events.try_recv().is_err());
}

#[test]
fn a_transport_that_cannot_be_made_says_why() {
    let file = discovery_file("curl");
    let (tx, events) = mpsc::channel();
    let make: MakeTransport = Box::new(|| Err("no curl".to_owned()));
    let _handle = start(settings(&file), make, FAST, move |event| {
        let _ = tx.send(event);
    })
    .unwrap();
    assert_eq!(event(&events), Event::Failed("no curl".to_owned()));
}
