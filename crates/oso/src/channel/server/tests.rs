//! The agent channel's server against a stand-in UI thread, over a real socket in a scratch
//! directory.
//!
//! Must not depend on: `app` or a window; the stand-in answers in their place.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use oso_platform::control::{ControlDir, ControlStream, Socket};
use serde_json::{Value, json};

use super::{Connection, Event, REFUSED_BY_THE_PERSON, Request, Shared, spawn};
use crate::channel::protocol::{Op, PROTOCOL_VERSION};
use crate::channel::wire::{request, response_result};

/// A scratch control directory with the channel listening in it, and what reaches the UI.
struct Running {
    dir: ControlDir,
    path: PathBuf,
    shared: Arc<Shared>,
    events: mpsc::Receiver<Request>,
}

impl Running {
    fn new() -> Self {
        // Each test runs on a thread of its own. Short: socket paths are limited to about
        // 100 bytes.
        let thread = format!("{:?}", std::thread::current().id());
        let n: String = thread.chars().filter(char::is_ascii_digit).collect();
        let path = std::env::temp_dir().join(format!("osoa-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        let dir = ControlDir::at(&path).unwrap();
        let listener = dir.listen(Socket::Agent).unwrap();
        let shared = Arc::new(Shared::new(7));
        let (sent, events) = mpsc::channel();
        spawn(listener, Arc::clone(&shared), move |request| {
            sent.send(request).is_ok()
        })
        .unwrap();
        Self {
            dir,
            path,
            shared,
            events,
        }
    }

    fn connect(&self) -> Client {
        let (_, stream) = self
            .dir
            .connect(Socket::Agent, Some(std::process::id()))
            .unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        Client {
            reader: BufReader::new(stream.try_clone().unwrap()),
            stream,
        }
    }

    fn next(&self) -> Request {
        self.events
            .recv_timeout(Duration::from_secs(5))
            .expect("an event")
    }

    /// The next event, which must be a connection asking; answered with `allow`.
    fn answer(&self, allow: bool) -> Connection {
        let request = self.next();
        assert!(
            matches!(request.event, Event::Asks(_)),
            "{:?}",
            request.event
        );
        let reply = if allow {
            Ok(Value::Null)
        } else {
            Err(REFUSED_BY_THE_PERSON.to_owned())
        };
        request.reply.unwrap().send(reply).unwrap();
        request.connection
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        let _ = self.dir.connect(Socket::Agent, Some(std::process::id()));
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

struct Client {
    stream: ControlStream,
    reader: BufReader<ControlStream>,
}

impl Client {
    fn send(&mut self, method: &str, params: Option<Value>) {
        writeln!(self.stream, "{}", request(1, method, params)).unwrap();
    }

    fn reply(&mut self) -> Result<Value, String> {
        let mut line = String::new();
        self.reader.read_line(&mut line).unwrap();
        response_result(&line)
    }
}

#[test]
fn a_probe_asks_nothing_and_a_first_request_asks_the_person() {
    let running = Running::new();
    drop(running.connect());
    let mut client = running.connect();
    client.send("version", None);
    let connection = running.answer(true);
    assert_eq!(connection.number, 1, "the probe raised no event");
    let version = client.reply().unwrap();
    assert_eq!(version["protocol"], PROTOCOL_VERSION);
    assert_eq!(version["pid"], std::process::id());
}

#[test]
fn requests_reach_the_ui_once_the_person_agrees() {
    let running = Running::new();
    let mut client = running.connect();
    client.send("state", None);
    running.answer(true);
    let request = running.next();
    assert!(
        matches!(request.event, Event::Op(Op::State { .. })),
        "{:?}",
        request.event
    );
    request
        .reply
        .unwrap()
        .send(Ok(json!({ "version": 1 })))
        .unwrap();
    assert_eq!(client.reply().unwrap()["version"], 1);
    client.send("send-keys", Some(json!({ "keys": "y" })));
    assert!(client.reply().unwrap_err().starts_with("unknown method"));
    drop(client);
    assert!(matches!(running.next().event, Event::Gone));
}

#[test]
fn a_refusal_holds_off_the_next_connection() {
    let running = Running::new();
    let mut client = running.connect();
    client.send("version", None);
    running.answer(false);
    assert_eq!(client.reply(), Err(REFUSED_BY_THE_PERSON.to_owned()));
    let mut next = running.connect();
    next.send("version", None);
    let refused = next.reply().unwrap_err();
    assert!(
        refused.contains("refused an agent a moment ago"),
        "{refused}"
    );
    assert!(
        running
            .events
            .recv_timeout(Duration::from_millis(300))
            .is_err()
    );
}

#[test]
fn one_connection_waits_at_a_time() {
    let running = Running::new();
    let mut first = running.connect();
    first.send("version", None);
    let asking = running.next();
    let mut second = running.connect();
    second.send("version", None);
    assert!(second.reply().unwrap_err().contains("another agent waits"));
    // While it waits, the first is told so, and nothing more reaches the UI.
    first.send("state", None);
    assert!(first.reply().unwrap_err().contains("not answered yet"));
    asking.reply.unwrap().send(Ok(Value::Null)).unwrap();
    assert_eq!(first.reply().unwrap()["protocol"], PROTOCOL_VERSION);
}

#[test]
fn a_client_that_goes_while_it_waits_closes_its_question() {
    let running = Running::new();
    let mut client = running.connect();
    client.send("version", None);
    let asking = running.next();
    assert!(matches!(asking.event, Event::Asks(_)));
    drop(client);
    let gone = running.next();
    assert_eq!(gone.connection, asking.connection);
    assert!(matches!(gone.event, Event::Gone));
}

#[test]
fn a_flood_of_requests_is_slowed_down() {
    let running = Running::new();
    let mut client = running.connect();
    client.send("version", None);
    running.answer(true);
    client.reply().unwrap();
    let mut slowed = 0;
    for _ in 0..60 {
        client.send("version", None);
        if client.reply().is_err() {
            slowed += 1;
        }
    }
    assert!(slowed > 0, "60 requests in a moment all went through");
}

#[test]
fn connections_carry_their_run() {
    let running = Running::new();
    let mut client = running.connect();
    client.send("version", None);
    assert_eq!(running.answer(true).run, 7);
}

#[test]
fn a_silent_client_is_closed_after_the_deadline() {
    let running = Running::new();
    let mut client = running.connect();
    let started = std::time::Instant::now();
    let mut line = String::new();
    assert_eq!(
        client.reader.read_line(&mut line).unwrap(),
        0,
        "closed: {line}"
    );
    assert!(started.elapsed() < super::FIRST_REQUEST + Duration::from_secs(2));
    // Noted for the person, once, but nothing asks.
    assert!(matches!(running.next().event, Event::Silent));
    assert!(
        running
            .events
            .recv_timeout(Duration::from_millis(100))
            .is_err()
    );
}

#[test]
fn a_waiting_client_that_floods_is_closed() {
    let running = Running::new();
    let mut client = running.connect();
    client.send("version", None);
    let asking = running.next();
    for _ in 0..20 {
        client.send("state", None);
    }
    let gone = running.next();
    assert_eq!(gone.connection, asking.connection);
    assert!(matches!(gone.event, Event::Gone));
}

#[test]
fn a_client_that_leaves_while_it_waits_locks_no_one_out() {
    let running = Running::new();
    let mut client = running.connect();
    client.send("version", None);
    let asking = running.next();
    drop(client);
    assert!(matches!(running.next().event, Event::Gone));
    drop(asking);
    let mut next = running.connect();
    next.send("version", None);
    running.answer(true);
    assert_eq!(next.reply().unwrap()["protocol"], PROTOCOL_VERSION);
}

#[test]
fn only_the_persons_refusal_holds_others_off() {
    let running = Running::new();
    let mut client = running.connect();
    client.send("version", None);
    let asking = running.next();
    asking
        .reply
        .unwrap()
        .send(Err("the person turned the agent channel off".to_owned()))
        .unwrap();
    assert!(client.reply().is_err());
    let mut next = running.connect();
    next.send("version", None);
    running.answer(true);
    assert!(next.reply().is_ok());
}

#[test]
fn connections_that_say_nothing_keep_no_agent_out() {
    let running = Running::new();
    let silent: Vec<Client> = (0..super::MAX_CLIENTS).map(|_| running.connect()).collect();
    let mut client = running.connect();
    client.send("version", None);
    running.answer(true);
    assert_eq!(client.reply().unwrap()["protocol"], PROTOCOL_VERSION);
    drop(silent);
}

/// The next event, which must be a claim of `session`; answered with `bridges`.
fn claim(running: &Running, session: &str, bridges: bool) -> Connection {
    let request = running.next();
    let Event::Claims {
        session: claimed, ..
    } = &request.event
    else {
        panic!("{:?}", request.event);
    };
    assert_eq!(claimed, session);
    request
        .reply
        .unwrap()
        .send(Ok(Value::Bool(bridges)))
        .unwrap();
    request.connection
}

#[test]
fn a_hosted_sessions_bridge_is_let_in_unasked_and_beside_the_others() {
    let running = Running::new();
    // Two connections the person let in hold the two slots.
    let mut held = Vec::new();
    for _ in 0..super::MAX_CLIENTS {
        let mut client = running.connect();
        client.send("version", None);
        running.answer(true);
        client.reply().unwrap();
        held.push(client);
    }
    let session = json!({ "session": "s-1" });
    let mut bridge = running.connect();
    bridge.send("version", Some(session.clone()));
    let connection = claim(&running, "s-1", true);
    assert_eq!(bridge.reply().unwrap()["protocol"], PROTOCOL_VERSION);
    bridge.send("state", None);
    let request = running.next();
    assert_eq!(request.connection, connection);
    assert!(matches!(request.event, Event::Op(Op::State { .. })));
    request.reply.unwrap().send(Ok(json!({}))).unwrap();
    bridge.reply().unwrap();
    drop(bridge);
    assert!(matches!(running.next().event, Event::Gone));
    // A claim the UI does not believe asks as any connection, and then needs a slot.
    let mut other = running.connect();
    other.send("version", Some(session));
    claim(&running, "s-1", false);
    assert!(other.reply().unwrap_err().contains("too many agents"));
    drop(held);
}

/// A session tool that asks the person holds its reply past the usual wait, answers "wait" to
/// what the client sends meanwhile, and ends when the client goes, so that the UI closes the
/// question (ADR 0121, step 6).
#[test]
fn a_call_that_asks_the_person_waits_for_them_and_ends_with_its_client() {
    let running = Running::new();
    let mut client = running.connect();
    client.send("version", None);
    let connection = running.answer(true);
    client.reply().unwrap();
    client.send(
        "start_worktree_session",
        Some(json!({ "branch": "x", "brief": "do it" })),
    );
    let call = running.next();
    assert!(
        matches!(call.event, Event::Op(Op::Session(_))),
        "{:?}",
        call.event
    );
    // The UI holds the reply, as the person has not answered.
    client.send("session_list", None);
    let waiting = client.reply().unwrap_err();
    assert!(waiting.starts_with("wait:"), "{waiting}");
    drop(client);
    let gone = running.next();
    assert_eq!(gone.connection, connection);
    assert!(matches!(gone.event, Event::Gone), "{:?}", gone.event);
    drop(call);
}

/// The person's answer comes in time, however long they took.
#[test]
fn a_call_that_asks_the_person_replies_with_what_the_ui_says() {
    let running = Running::new();
    let mut client = running.connect();
    client.send("version", None);
    running.answer(true);
    client.reply().unwrap();
    client.send("session_start", None);
    let call = running.next();
    std::thread::sleep(Duration::from_millis(300));
    call.reply
        .unwrap()
        .send(Ok(json!({ "session": { "name": "x" } })))
        .unwrap();
    assert_eq!(client.reply().unwrap()["session"]["name"], "x");
}
