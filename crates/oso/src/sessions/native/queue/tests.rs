//! The queue's rules (ADR 0165, step 2, Q1 (a)): when a message goes, waits, comes back or is
//! refused, and its rows.
//!
//! Must not depend on: the app; the tests say where the session is.

use super::{MAX_MESSAGE, MAX_QUEUED, Move, Pause, Queue, Refusal, Run, Sent, Turn};
use crate::sessions::native::render::queue_rows;

const IDLE: Turn = Turn {
    busy: false,
    run: Run::Running,
    leaving: false,
};
const BUSY: Turn = Turn { busy: true, ..IDLE };
const STARTING: Turn = Turn {
    busy: true,
    run: Run::Starting,
    leaving: false,
};
const STOPPED: Turn = Turn {
    busy: false,
    run: Run::Ended,
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
fn a_message_sent_while_the_session_starts_waits_unpaused_for_its_first_turn() {
    let mut queue = Queue::default();
    assert_eq!(queue.send("a".into(), STARTING), Sent::Queued(1));
    assert_eq!(queue.paused(), None);
    assert_eq!(queue.idle(STARTING), None);
    assert_eq!(queue.idle(IDLE).as_deref(), Some("a"));
}

#[test]
fn empty_long_and_one_too_many_are_refused_and_change_nothing() {
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
    queue.failed(None);
    assert_eq!(
        queue.send("late".into(), BUSY),
        Sent::Refused(Refusal::Full)
    );
    assert_eq!(
        queue.paused(),
        Some(Pause::Held),
        "a refusal keeps the pause"
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
fn the_message_edited_taken_back_or_dropped_ends_the_edit() {
    let mut queue = Queue::default();
    let a = queued(&mut queue, "a");
    let b = queued(&mut queue, "b");
    queue.edit(a).unwrap();
    assert_eq!(queue.take_back(a).as_deref(), Some("a"));
    assert_eq!(queue.editing(), None);
    queue.edit(b).unwrap();
    assert!(queue.drop_one(b));
    assert_eq!(queue.editing(), None);
    assert_eq!(queue.send("c".into(), IDLE), Sent::Now("c".into()));
}

#[test]
fn a_failed_delivery_pauses_and_the_queues_own_goes_back_first() {
    let mut queue = Queue::default();
    queued(&mut queue, "a");
    queued(&mut queue, "b");
    let first = queue.idle(IDLE).unwrap();
    queue.failed(Some(first));
    assert_eq!(texts(&queue), ["a", "b"]);
    assert_eq!(queue.paused(), Some(Pause::Held));
    assert_eq!(queue.idle(IDLE), None);
    // The person's own text failing pauses too: nothing queued goes ahead of it.
    let Sent::Now(_) = queue.send("mine".into(), IDLE) else {
        panic!("sent now")
    };
    queue.failed(None);
    assert_eq!(texts(&queue), ["a", "b"]);
    assert_eq!(queue.idle(IDLE), None);
}

#[test]
fn a_paused_queue_goes_on_with_the_persons_send_or_an_empty_one() {
    let mut queue = Queue::default();
    queued(&mut queue, "a");
    queue.ended();
    assert_eq!(queue.paused(), Some(Pause::Ended));
    // Still stopped: a send waits, and the queue stays paused.
    assert_eq!(queue.send("b".into(), STOPPED), Sent::Queued(2));
    assert_eq!(queue.paused(), Some(Pause::Ended));
    assert_eq!(
        queue.send("".into(), STOPPED),
        Sent::Refused(Refusal::Empty)
    );
    // Running again and busy with a peer's turn: an empty send resumes, and writes nothing.
    assert_eq!(queue.send("".into(), BUSY), Sent::Resumed);
    assert_eq!(queue.paused(), None);
    assert_eq!(queue.idle(IDLE).as_deref(), Some("a"));
    // Paused and idle: an empty send writes the next at once.
    queue.ended();
    assert_eq!(queue.send("".into(), IDLE), Sent::Next("b".into()));
    // A message the person sends while paused and idle goes, and the rest with it.
    queued(&mut queue, "c");
    queue.failed(None);
    assert_eq!(queue.send("d".into(), IDLE), Sent::Now("d".into()));
    assert_eq!(queue.paused(), None);
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

#[test]
fn a_message_is_moved_as_far_as_an_end_by_its_id() {
    let mut queue = Queue::default();
    let a = queued(&mut queue, "a");
    queued(&mut queue, "b");
    let c = queued(&mut queue, "c");
    assert_eq!(queue.shift(c, Move::Down), None, "at the bottom already");
    assert_eq!(queue.shift(c, Move::Up), Some(1));
    assert_eq!(texts(&queue), ["a", "c", "b"]);
    assert_eq!(queue.shift(a, Move::Up), None, "at the top already");
}

#[test]
fn the_rows_name_each_message_by_its_first_line_and_say_why_none_goes() {
    let mut queue = Queue::default();
    assert_eq!(queue_rows(&queue), "");
    let a = queued(&mut queue, "fix the tests\nthen push");
    queued(&mut queue, "short");
    queue.edit(a).unwrap();
    let rows = queue_rows(&queue);
    let lines: Vec<&str> = rows.lines().collect();
    assert_eq!(
        lines,
        ["Queued (2)", "  1. fix the tests… (editing)", "  2. short"]
    );
    queue.give_up_edit();
    queue.ended();
    assert!(queue_rows(&queue).starts_with("Queued (2) · the session stopped"));
    queue.failed(None);
    assert!(queue_rows(&queue).starts_with("Queued (2) · paused"));
}

#[test]
fn a_resume_keeps_the_queue_waiting_for_an_empty_send() {
    let mut queue = Queue::default();
    queued(&mut queue, "a");
    queue.ended();
    queue.resumed();
    assert_eq!(queue.paused(), Some(Pause::Held));
    assert_eq!(queue.idle(IDLE), None, "still the person's to go on");
    assert_eq!(queue.send("".into(), IDLE), Sent::Next("a".into()));
    // A queue that was not paused stays so.
    queue.resumed();
    assert_eq!(queue.paused(), None);
}

#[test]
fn an_interrupt_pauses_the_queue_until_the_person_sends() {
    let mut queue = Queue::default();
    queue.interrupted();
    assert_eq!(queue.paused(), None, "an empty queue has nothing to hold");
    queued(&mut queue, "a");
    queued(&mut queue, "b");
    queue.interrupted();
    assert_eq!(queue.paused(), Some(Pause::Held));
    assert_eq!(queue.idle(IDLE), None);
    assert_eq!(queue.send(String::new(), IDLE), Sent::Next("a".into()));
    assert_eq!(queue.idle(IDLE), Some("b".into()));
}

#[test]
fn a_send_now_goes_ahead_of_the_queue_which_then_goes_on() {
    let mut queue = Queue::default();
    queued(&mut queue, "a");
    queue.interrupted();
    assert_eq!(queue.send_now("now".into(), BUSY), Sent::Ahead(2));
    assert_eq!(texts(&queue), ["now", "a"]);
    assert_eq!(queue.paused(), None);
    assert_eq!(queue.idle(BUSY), None, "once the interrupted turn ends");
    assert_eq!(queue.idle(IDLE), Some("now".into()));
    assert_eq!(queue.idle(IDLE), Some("a".into()));
}

#[test]
fn a_send_now_with_no_turn_to_interrupt_is_a_send() {
    let mut queue = Queue::default();
    assert_eq!(queue.send_now("a".into(), IDLE), Sent::Now("a".into()));
    assert_eq!(queue.send_now("b".into(), STARTING), Sent::Queued(1));
    assert_eq!(queue.send_now("c".into(), STOPPED), Sent::Queued(2));
    assert_eq!(texts(&queue), ["b", "c"], "behind what waits");
}

#[test]
fn a_send_now_keeps_the_limits_and_waits_for_an_edit() {
    let mut queue = Queue::default();
    assert_eq!(
        queue.send_now(" \n".into(), BUSY),
        Sent::Refused(Refusal::Empty)
    );
    let long = "x".repeat(MAX_MESSAGE + 1);
    assert_eq!(queue.send_now(long, BUSY), Sent::Refused(Refusal::TooLong));
    let id = queued(&mut queue, "a");
    queue.edit(id);
    assert_eq!(
        queue.send_now("b".into(), BUSY),
        Sent::Refused(Refusal::Editing)
    );
    queue.give_up_edit();
    for n in 1..MAX_QUEUED {
        queued(&mut queue, &n.to_string());
    }
    assert_eq!(
        queue.send_now("b".into(), BUSY),
        Sent::Refused(Refusal::Full)
    );
    assert_eq!(queue.waiting().len(), MAX_QUEUED);
}

#[test]
fn in_the_apps_order_a_send_now_is_not_paused_by_its_own_interrupt() {
    // The app puts it first, then interrupts without pausing: the queue goes on (Q1 (a)).
    let mut queue = Queue::default();
    queued(&mut queue, "a");
    assert_eq!(queue.send_now("now".into(), BUSY), Sent::Ahead(2));
    assert_eq!(queue.paused(), None);
    assert_eq!(queue.idle(IDLE), Some("now".into()));
    assert_eq!(queue.idle(IDLE), Some("a".into()));
    // The person's `C-c C-k` pauses it.
    queued(&mut queue, "b");
    queue.interrupted();
    assert_eq!(queue.idle(IDLE), None);
}
