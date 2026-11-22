//! The permission requests' queue, with calls the tests say are found or not.
//!
//! Must not depend on: the app; the tests stand in for its look at the rows.

use std::time::{Duration, Instant};

use super::{Ask, Asks, Denied, FIND_WITHIN, Look, Outcome, Question, Refused};
use crate::channel::Connection;
use crate::channel::protocol::permission::Call;
use crate::sessions::native::card::Card;

fn ask(from: u64, key: u64, id: &str, came: Instant) -> Ask<()> {
    Ask {
        from: Connection {
            run: 1,
            number: from,
        },
        key,
        call: Call {
            tool_name: "Bash".into(),
            tool_use_id: id.into(),
            input: None,
        },
        came,
        question: None,
        shown: None,
        reply: (),
    }
}

fn question(label: &str) -> Question {
    Question {
        card: Card::Call {
            title: label.into(),
            lines: Vec::new(),
        },
        details: String::new(),
        whole: false,
    }
}

fn denied(outcomes: &[Outcome<()>]) -> Vec<(String, Denied)> {
    outcomes
        .iter()
        .filter_map(|outcome| match outcome {
            Outcome::Denied(ask, why, _) => Some((ask.call.tool_use_id.clone(), *why)),
            Outcome::Found(_) => None,
        })
        .collect()
}

/// Pushes `ask` unless it is refused, and says why it is.
fn push(asks: &mut Asks<()>, ask: Ask<()>) -> Option<Refused> {
    let refused = asks.refuses(ask.from, ask.key, &ask.call.tool_use_id);
    if refused.is_none() {
        asks.push(ask);
    }
    refused
}

#[test]
fn a_connection_waits_for_one_answer_and_a_call_is_asked_once() {
    let now = Instant::now();
    let mut asks = Asks::default();
    assert_eq!(push(&mut asks, ask(1, 7, "t1", now)), None);
    assert_eq!(push(&mut asks, ask(1, 7, "t2", now)), Some(Refused::Twice));
    assert_eq!(push(&mut asks, ask(2, 7, "t1", now)), Some(Refused::Again));
    // Another session's call of the same id is its own.
    assert_eq!(push(&mut asks, ask(2, 8, "t1", now)), None);
}

#[test]
fn a_call_that_does_not_show_in_time_is_denied_and_one_found_is_asked_in_turn() {
    let start = Instant::now();
    let mut asks = Asks::default();
    asks.push(ask(1, 7, "t1", start));
    asks.push(ask(2, 8, "t2", start));
    assert_eq!(asks.deadline(), Some(start + FIND_WITHIN));
    let found = |ask: &Ask<()>| match ask.call.tool_use_id.as_str() {
        "t2" => Look::Found(question("run make?")),
        _ => Look::NotWaiting,
    };
    let outcomes = asks.look(start + Duration::from_millis(10), found);
    assert!(matches!(outcomes.as_slice(), [Outcome::Found(8)]));
    let late = asks.look(start + FIND_WITHIN, found);
    assert_eq!(denied(&late), [("t1".to_owned(), Denied::NotFound)]);
    assert_eq!(asks.deadline(), None, "only found ones wait");
    let next = asks.unshown().next().unwrap();
    assert!(
        matches!(&next.question.as_ref().unwrap().card, Card::Call { title, .. } if title == "run make?")
    );
    next.shown = Some(start);
    assert!(asks.unshown().next().is_none(), "its card is shown");
    assert!(asks.shown(7).is_none(), "another session's");
    let answered = asks.take_shown(8).unwrap();
    assert_eq!(answered.call.tool_use_id, "t2");
    assert!(asks.is_empty());
    // Asked about once.
    assert_eq!(
        push(&mut asks, ask(2, 8, "t2", start)),
        Some(Refused::Again)
    );
}

#[test]
fn a_found_call_that_stops_waiting_or_differs_is_denied_before_it_is_asked() {
    let now = Instant::now();
    let mut asks = Asks::default();
    asks.push(ask(1, 7, "t1", now));
    asks.push(ask(2, 8, "t2", now));
    asks.push(ask(3, 9, "t3", now));
    asks.look(now, |ask| match ask.call.tool_use_id.as_str() {
        "t3" => Look::Differs,
        _ => Look::Found(question("q")),
    });
    let outcomes = asks.look(now, |ask| match ask.key {
        7 => Look::NotWaiting,
        8 => Look::Gone,
        _ => Look::Waits,
    });
    let mut said = denied(&outcomes);
    said.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        said,
        [
            ("t1".to_owned(), Denied::NoLonger),
            ("t2".to_owned(), Denied::Gone)
        ]
    );
    assert!(asks.is_empty());
}

#[test]
fn a_gone_connections_requests_leave() {
    let now = Instant::now();
    let mut asks = Asks::default();
    asks.push(ask(1, 7, "t1", now));
    asks.push(ask(2, 8, "t2", now));
    assert!(asks.remove_where(|ask| ask.key == 99).is_empty());
    let gone = asks.remove_where(|ask| ask.from.number == 1);
    assert_eq!(gone.len(), 1);
    assert!(!asks.is_empty());
}

/// What the queue costs each key with a full table of 256 sessions, as the app looks for each
/// request's call in two maps, its session's rows and their calls: run with `--ignored
/// --nocapture` to see it (ADR 0165, step 2, measured for the PR).
#[test]
#[ignore = "a measurement"]
#[expect(
    clippy::disallowed_macros,
    reason = "the measurement prints what it measured"
)]
fn what_the_queue_costs_a_key() {
    use std::collections::HashMap;
    let now = Instant::now();
    let sessions: HashMap<u64, HashMap<String, ()>> = (0..256)
        .map(|key| (key, (0..64).map(|n| (format!("u{n}"), ())).collect()))
        .collect();
    for waiting in [0, 1, 16, 256] {
        let mut asks = Asks::default();
        for n in 0..waiting {
            asks.push(ask(n, n, &format!("t{n}"), now));
        }
        let rounds = 10_000u32;
        let started = Instant::now();
        for _ in 0..rounds {
            let outcomes = asks.look(now, |ask| {
                match sessions
                    .get(&ask.key)
                    .and_then(|calls| calls.get(&ask.call.tool_use_id))
                {
                    Some(()) => Look::Waits,
                    None => Look::NotWaiting,
                }
            });
            assert!(outcomes.is_empty());
            assert!(asks.unshown().next().is_none());
        }
        let each = started.elapsed() / rounds;
        println!("{waiting} waiting: {each:?} a key");
    }
}
