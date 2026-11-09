//! Cancels, the agent going, and lines that find no room, against a channel whose calls wait.
//!
//! Must not depend on: a socket; [`Waiting`] stands in for a call that waits for the person.

use std::collections::VecDeque;
use std::io::{BufReader, Cursor, Read};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use super::super::stdin::{Interrupt, QUEUED};
use super::super::{Bridge, Connect, Failed, Link, serve};
use super::{Waited, Waiting, call};

/// A step of the agent's stdin.
enum Step {
    Line(Value),
    /// Waits until the call in flight waits on the channel.
    Started,
    /// Notes whether the call's connection has been closed by now.
    Closed(Arc<Mutex<Option<bool>>>),
}

/// stdin that goes step by step: each read comes after the thread handled the line before.
struct Script {
    steps: VecDeque<Step>,
    line: Cursor<Vec<u8>>,
    waited: Waited,
}

impl Read for Script {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            let read = self.line.read(buf)?;
            if read > 0 {
                return Ok(read);
            }
            match self.steps.pop_front() {
                None => return Ok(0),
                Some(Step::Line(line)) => self.line = Cursor::new(format!("{line}\n").into_bytes()),
                Some(Step::Started) => {
                    let (state, woken) = &*self.waited;
                    let guard = state.lock().unwrap();
                    drop(woken.wait_while(guard, |(started, _)| !*started).unwrap());
                }
                Some(Step::Closed(seen)) => {
                    *seen.lock().unwrap() = Some(self.waited.0.lock().unwrap().1);
                }
            }
        }
    }
}

fn cancel(id: u64) -> Value {
    json!({ "jsonrpc": "2.0", "method": "notifications/cancelled", "params": { "requestId": id } })
}

fn ping(id: u64) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": "ping" })
}

/// Serves `steps` with a bridge whose every connection waits, and returns the response lines
/// and how many connections were made. The end of stdin closes nothing, so that only a cancel
/// can end a call.
fn serve_waiting(steps: Vec<Step>) -> (Vec<Value>, usize, Waited) {
    let waited: Waited = Arc::default();
    let link = Arc::clone(&waited);
    let connects = Arc::new(Mutex::new(0));
    let counted = Arc::clone(&connects);
    let connect: Connect = Box::new(move || {
        *counted.lock().unwrap() += 1;
        Ok(Box::new(Waiting(Arc::clone(&link))) as Box<dyn Link>)
    });
    let mut bridge = Bridge::new(connect);
    let input = Script {
        steps: steps.into(),
        line: Cursor::default(),
        waited: Arc::clone(&waited),
    };
    let mut output = Vec::new();
    let input = Box::new(BufReader::new(input));
    assert_eq!(serve(&mut bridge, input, false, &mut output), 0);
    let lines = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let connects = *connects.lock().unwrap();
    (lines, connects, waited)
}

#[test]
fn a_cancel_closes_only_the_call_it_names_and_that_call_gets_no_response() {
    let seen = Arc::new(Mutex::new(None));
    let (lines, connects, waited) = serve_waiting(vec![
        Step::Line(call("change", &json!({ "name": "insert", "arg": "x" }))),
        Step::Started,
        Step::Line(cancel(8)),
        Step::Closed(Arc::clone(&seen)),
        Step::Line(cancel(7)),
    ]);
    assert_eq!(
        *seen.lock().unwrap(),
        Some(false),
        "a cancel of another request left the call waiting"
    );
    assert!(waited.0.lock().unwrap().1, "its own cancel closed it");
    assert!(
        lines.is_empty(),
        "a cancelled call is not answered: {lines:?}"
    );
    assert_eq!(connects, 1);
}

#[test]
fn a_request_cancelled_while_it_waits_its_turn_never_runs() {
    let mut second = call("look", &json!({ "name": "forward-word" }));
    second["id"] = json!(9);
    let (lines, connects, _) = serve_waiting(vec![
        Step::Line(call("change", &json!({ "name": "insert", "arg": "x" }))),
        Step::Started,
        Step::Line(second),
        Step::Line(ping(10)),
        Step::Line(cancel(9)),
        Step::Line(cancel(7)),
    ]);
    let ids: Vec<&Value> = lines.iter().map(|line| &line["id"]).collect();
    assert_eq!(ids, [&json!(10)], "{lines:?}");
    assert_eq!(connects, 1, "the cancelled request made no connection");
}

#[test]
fn lines_beyond_the_queue_are_refused_and_a_cancel_still_gets_through() {
    let mut steps = vec![
        Step::Line(call("change", &json!({ "name": "insert", "arg": "x" }))),
        Step::Started,
    ];
    steps.extend((100..).take(QUEUED + 2).map(|id| Step::Line(ping(id))));
    steps.push(Step::Line(cancel(7)));
    let (lines, _, waited) = serve_waiting(steps);
    assert!(waited.0.lock().unwrap().1, "the cancel came through");
    let answered = lines.iter().filter(|line| line.get("result").is_some());
    assert_eq!(answered.count(), QUEUED);
    let refused: Vec<&Value> = lines
        .iter()
        .filter(|line| line["error"]["code"] == -32000)
        .map(|line| &line["id"])
        .collect();
    assert_eq!(refused, [&json!(100 + QUEUED), &json!(101 + QUEUED)]);
}

#[test]
fn the_agent_going_while_a_call_connects_ends_that_call() {
    let waited: Waited = Arc::default();
    let mut bridge = Bridge::new(Box::new(|| Err("unused".into())));
    let interrupt = Arc::clone(&bridge.interrupt);
    let link = Arc::clone(&waited);
    bridge.connect = Box::new(move || {
        // stdin ends after the bridge checked for it, and before the connection can be closed.
        interrupt.end();
        Ok(Box::new(Waiting(Arc::clone(&link))) as Box<dyn Link>)
    });
    let line = bridge
        .respond(&call("state", &json!({})).to_string())
        .unwrap();
    let answered: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(answered["result"]["isError"], true);
    assert!(
        !waited.0.lock().unwrap().0,
        "nothing waited on the connection"
    );
}

/// A channel whose call the agent cancels just as it answers.
struct CancelledAsItEnds(Arc<Interrupt>, Arc<Mutex<usize>>);

impl Link for CancelledAsItEnds {
    fn call(&mut self, method: &str, _: Value) -> Result<Value, Failed> {
        if method == "version" {
            return Ok(json!({ "protocol": crate::channel::protocol::PROTOCOL_VERSION }));
        }
        self.0.cancel(json!(7));
        Ok(json!({}))
    }

    fn closer(&self) -> Option<Box<dyn Fn() + Send>> {
        let closed = Arc::clone(&self.1);
        Some(Box::new(move || *closed.lock().unwrap() += 1))
    }
}

#[test]
fn a_cancel_that_lands_as_the_call_ends_makes_the_next_call_connect_again() {
    let closed = Arc::new(Mutex::new(0));
    let connects = Arc::new(Mutex::new(0));
    let mut bridge = Bridge::new(Box::new(|| Err("unused".into())));
    let interrupt = Arc::clone(&bridge.interrupt);
    let (shut, counted) = (Arc::clone(&closed), Arc::clone(&connects));
    bridge.connect = Box::new(move || {
        *counted.lock().unwrap() += 1;
        let link = CancelledAsItEnds(Arc::clone(&interrupt), Arc::clone(&shut));
        Ok(Box::new(link) as Box<dyn Link>)
    });
    let request = call("state", &json!({})).to_string();
    assert!(bridge.respond(&request).is_none(), "cancelled: no response");
    assert_eq!(*closed.lock().unwrap(), 1);
    let mut next = call("state", &json!({}));
    next["id"] = json!(8);
    assert!(bridge.respond(&next.to_string()).is_some());
    assert_eq!(
        *connects.lock().unwrap(),
        2,
        "the closed connection was dropped"
    );
}
