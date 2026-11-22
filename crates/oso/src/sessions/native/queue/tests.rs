//! The queue's rules (ADR 0165, step 2, Q1 (a)): when a message goes, waits, comes back or is
//! refused.
//!
//! Must not depend on: the app; the tests say whether a turn runs.

use super::{MAX_MESSAGE, MAX_QUEUED, Move, Queue, Refusal, Sent, Turn};

const IDLE: Turn = Turn {
    busy: false,
    running: true,
    leaving: false,
};
const BUSY: Turn = Turn {
    busy: true,
    running: true,
    leaving: false,
};
const STOPPED: Turn = Turn {
    busy: false,
    running: false,
    leaving: false,
};

fn texts(queue: &Queue) -> Vec<&str> {
    queue.waiting().map(|queued| queued.text.as_str()).collect()
}

fn queued(queue: &mut Queue, text: &str) -> u64 {
    assert!(matches!(queue.send(text.into(), BUSY), Sent::Queued(_)));
    queue.waiting().last().unwrap().id
}

#[test]
fn an_idle_session_takes_a_message_at_once_and_a_busy_one_queues_it() {
    let mut queue = Queue::default();
    assert_eq!(queue.send("a".into(), IDLE), Sent::Now("a".into()));
    assert_eq!(queue.send("b".into(), BUSY), Sent::Queued(1));
    assert_eq!(queue.idle(BUSY), None, "only once the turn ends");
    assert_eq!(queue.idle(IDLE).as_deref(), Some("b"));
    assert!(queue.is_empty());
}

#[test]
fn empty_long_and_one_too_many_are_refused() {
    let mut queue = Queue::default();
    assert_eq!(
        queue.send(" \n".into(), IDLE),
        Sent::Refused(Refusal::Empty)
    );
    let long = "x".repeat(MAX_MESSAGE + 1);
    assert_eq!(queue.send(long, IDLE), Sent::Refused(Refusal::TooLong));
    for n in 0..MAX_QUEUED {
        queued(&mut queue, &n.to_string());
    }
    assert_eq!(
        queue.send("late".into(), BUSY),
        Sent::Refused(Refusal::Full)
    );
}

#[test]
fn an_edit_put_back_keeps_the_place_and_the_limits() {
    let mut queue = Queue::default();
    let a = queued(&mut queue, "a");
    queued(&mut queue, "b");
    assert_eq!(queue.edit(a).as_deref(), Some("a"));
    assert_eq!(queue.idle(IDLE), None, "the first is being edited");
    assert_eq!(queue.send("".into(), IDLE), Sent::Refused(Refusal::Empty));
    let long = "x".repeat(MAX_MESSAGE + 1);
    assert_eq!(queue.send(long, IDLE), Sent::Refused(Refusal::TooLong));
    assert_eq!(queue.send("a, better".into(), IDLE), Sent::PutBack);
    assert_eq!(texts(&queue), ["a, better", "b"]);
    // Idle again, the caller asks for the next: the edit held the queue back meanwhile.
    assert_eq!(queue.idle(IDLE).as_deref(), Some("a, better"));
}

#[test]
fn a_failed_delivery_goes_back_first_and_pauses() {
    let mut queue = Queue::default();
    queued(&mut queue, "a");
    queued(&mut queue, "b");
    let first = queue.idle(IDLE).unwrap();
    queue.failed(first);
    assert_eq!(texts(&queue), ["a", "b"]);
    assert!(queue.paused());
    assert_eq!(queue.idle(IDLE), None);
}

#[test]
fn a_paused_queue_goes_on_with_the_persons_send_or_an_empty_one() {
    let mut queue = Queue::default();
    queued(&mut queue, "a");
    queue.ended();
    assert!(queue.paused());
    // Still stopped: a send waits, and the queue stays paused.
    assert_eq!(queue.send("b".into(), STOPPED), Sent::Queued(2));
    assert!(queue.paused());
    assert_eq!(
        queue.send("".into(), STOPPED),
        Sent::Refused(Refusal::Empty)
    );
    // Running again and busy with a peer's turn: an empty send resumes, and writes nothing.
    assert_eq!(queue.send("".into(), BUSY), Sent::Resumed);
    assert!(!queue.paused());
    assert_eq!(queue.idle(IDLE).as_deref(), Some("a"));
    // Paused and idle: an empty send writes the next at once.
    queue.ended();
    assert_eq!(queue.send("".into(), IDLE), Sent::Now("b".into()));
}

#[test]
fn a_message_is_taken_back_dropped_and_moved_by_its_id() {
    let mut queue = Queue::default();
    let a = queued(&mut queue, "a");
    let b = queued(&mut queue, "b");
    let c = queued(&mut queue, "c");
    assert_eq!(queue.shift(c, Move::Up), Some(1));
    assert_eq!(texts(&queue), ["a", "c", "b"]);
    assert_eq!(queue.shift(a, Move::Up), None, "at the top already");
    assert!(queue.drop_one(b));
    assert!(!queue.drop_one(b), "gone");
    assert_eq!(queue.take_back(a).as_deref(), Some("a"));
    assert_eq!(texts(&queue), ["c"]);
}

#[test]
fn the_rows_name_each_message_by_its_first_line() {
    let mut queue = Queue::default();
    assert_eq!(queue.rows(), "");
    let a = queued(&mut queue, "fix the tests\nthen push");
    queued(&mut queue, "short");
    queue.edit(a).unwrap();
    let rows = queue.rows();
    let lines: Vec<&str> = rows.lines().collect();
    assert_eq!(
        lines,
        ["Queued (2)", "  1. fix the tests… (editing)", "  2. short"]
    );
    queue.give_up_edit();
    queue.ended();
    assert!(queue.rows().starts_with("Queued (2) · paused"));
}

#[test]
fn nothing_queued_goes_while_oso_waits_to_exit_but_the_persons_send_does() {
    let mut queue = Queue::default();
    queued(&mut queue, "a");
    let leaving = Turn {
        leaving: true,
        ..IDLE
    };
    assert_eq!(queue.idle(leaving), None);
    assert_eq!(queue.send("b".into(), leaving), Sent::Now("b".into()));
    assert_eq!(queue.idle(IDLE).as_deref(), Some("a"));
}
