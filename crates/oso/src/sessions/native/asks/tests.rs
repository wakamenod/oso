//! The permission requests' queue, with calls the tests say are found or not.
//!
//! Must not depend on: the app; the tests stand in for its look at the rows.

use std::time::{Duration, Instant};

use serde_json::json;

use super::{Ask, Asks, Block, Denied, FIND_WITHIN, Look, Outcome, Question, Refused, RequestId};
use crate::channel::Connection;
use crate::channel::protocol::permission::Call;
use crate::sessions::native::card::Card;

fn ask(from: u64, key: u64, id: &str, came: Instant) -> Ask<()> {
    Ask {
        id: RequestId::default(),
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
        made: None,
        asked: None,
        shown: None,
        reply: (),
    }
}

fn question(label: &str) -> Question {
    Question::new(Card::Plan {
        plan: label.into(),
        cut: false,
    })
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
fn a_sessions_requests_on_two_connections_are_shown_one_at_a_time_in_turn() {
    let start = Instant::now();
    let mut asks = Asks::default();
    asks.push(ask(1, 7, "t1", start));
    asks.push(ask(2, 7, "t2", start));
    assert!(asks.asked().next().is_none(), "none is found yet");
    let found = |ask: &Ask<()>| Look::Found(question(&ask.call.tool_use_id));
    asks.look(start, found);
    let first: Vec<String> = (asks.unshown())
        .map(|a| a.call.tool_use_id.clone())
        .collect();
    assert_eq!(first, ["t1"]);
    // Both wait for the person, the one shown and the one behind it, until answered.
    let asked =
        |asks: &Asks<()>| -> Vec<String> { asks.asked().map(|(_, id)| id.to_owned()).collect() };
    assert_eq!(asked(&asks), ["t1", "t2"]);
    asks.unshown().next().unwrap().shown = Some(start);
    // The second waits while the first card is shown, and is not shown with it.
    assert!(asks.unshown().next().is_none());
    assert_eq!(asks.take_shown(7).unwrap().call.tool_use_id, "t1");
    assert_eq!(asked(&asks), ["t2"]);
    let next: Vec<String> = (asks.unshown())
        .map(|a| a.call.tool_use_id.clone())
        .collect();
    assert_eq!(next, ["t2"]);
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
        matches!(&next.question().unwrap().card, Card::Plan { plan, .. } if plan == "run make?")
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
fn a_call_is_shown_by_the_block_made_of_it_and_denied_with_none() {
    let now = Instant::now();
    let mut asks = Asks::default();
    let block = Block::call("Bash", Some(&json!({"command": "ls"})), now);
    let mut made = ask(1, 7, "t1", now);
    made.made = Some(Box::new(block.clone()));
    asks.push(made);
    asks.push(ask(2, 7, "t2", now));
    asks.push(ask(3, 8, "t3", now));
    assert_eq!(asks.count(7), 0);
    let outcomes = asks.look(now, |ask| match ask.key {
        7 => Look::Call,
        _ => Look::Found(question("q")),
    });
    assert_eq!(denied(&outcomes), [("t2".to_owned(), Denied::Unreadable)]);
    // Found, it is as it was made.
    asks.look(now, |_| Look::Call);
    assert_eq!((asks.count(7), asks.count(8)), (1, 1));
    for ask in asks.unshown() {
        ask.shown = Some(now);
    }
    let shown = asks.shown(7).unwrap();
    assert_eq!(shown.request().unwrap().block, block);
    assert!(shown.question().is_none() && shown.made.is_none());
    let card = asks.shown(8).unwrap();
    assert!(card.request().is_none() && card.question().is_some());
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

#[test]
fn each_request_taken_gets_an_id_of_osos_that_reads_back() {
    let now = Instant::now();
    let mut asks = Asks::default();
    asks.push(ask(1, 7, "t1", now));
    asks.push(ask(2, 8, "t1", now));
    let found = |_: &Ask<()>| Look::Found(question("q"));
    asks.look(now, found);
    for ask in asks.unshown() {
        ask.shown = Some(now);
    }
    let ids: Vec<String> = [7, 8]
        .into_iter()
        .map(|key| asks.shown(key).unwrap().id.to_string())
        .collect();
    assert_eq!(ids, ["r1", "r2"]);
    assert_eq!(RequestId::parse("r2"), Some(asks.shown(8).unwrap().id));
    for text in [
        "",
        "r",
        "2",
        "r-1",
        "r+1",
        "r01",
        "R1",
        "r1 ",
        " r1",
        "r1x",
        "r99999999999999999999",
    ] {
        assert_eq!(RequestId::parse(text), None, "{text:?}");
    }
    assert_eq!(RequestId::parse("r0"), Some(RequestId::default()));
    // An answer keeps its call as answered lately, by whomever it came from.
    assert!(!asks.answered_lately(7, "t1"));
    asks.take_shown(7);
    assert!(asks.answered_lately(7, "t1"));
    assert!(!asks.answered_lately(8, "t1"));
}
