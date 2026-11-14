//! The review thread's queue: carries wait aside in order across a newer request, a carry is
//! never counted as waiting, and each carry is answered as failed, in order, once the root is
//! revoked.
//!
//! Must not depend on: `app`, `dispatch` or winit; these drive the queue and the thread
//! directly, and run no git.

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use oso_core::review::{Base, Review, Shape};

use super::tests::scratch_dir;
use super::{
    Asked, Report, Request, Reviewer, TrustedRoot, Waiting, next_request, pull, send_request,
};

/// A carry of the comments on `path`, of which a review with none has nothing to carry.
fn carry(path: &[u8]) -> Request {
    let shape = Arc::new(Shape::new(Base::Unknown, Vec::new()).unwrap());
    Request::Carry(Review::new().carry(path, shape))
}

/// What `request` is: `carry a` for a carry of `a`'s comments, else its kind.
fn what(request: Option<Request>) -> String {
    match request {
        None => "none".into(),
        Some(Request::Carry(carry)) => {
            let shown = format!("{carry:?}");
            let path = shown
                .strip_prefix("Carry { path: [")
                .and_then(|rest| rest.split(']').next())
                .unwrap_or_default();
            let bytes: Vec<u8> = path.split(", ").filter_map(|b| b.parse().ok()).collect();
            format!("carry {}", String::from_utf8_lossy(&bytes))
        }
        Some(Request::Open(_)) => "open".into(),
        Some(Request::Refresh { first }) => format!("refresh {}", first.len()),
        Some(Request::Baseline) => "baseline".into(),
    }
}

struct Queue {
    send: Sender<Request>,
    queue: Receiver<Request>,
    held: RefCell<Waiting>,
    waiting: AtomicUsize,
}

impl Queue {
    fn new() -> Queue {
        let (send, queue) = mpsc::channel();
        Queue {
            send,
            queue,
            held: RefCell::new(Waiting::default()),
            waiting: AtomicUsize::new(0),
        }
    }

    fn ask(&self, request: Request) {
        assert!(send_request(&self.send, &self.waiting, request));
    }

    fn next(&self) -> String {
        what(next_request(&self.queue, &self.held, &self.waiting))
    }

    /// What the running request's carries take: the queue pulled, then the oldest held.
    fn carry_between_files(&self) -> String {
        pull(&self.queue, &self.held, &self.waiting);
        what(
            self.held
                .borrow_mut()
                .carries
                .pop_front()
                .map(Request::Carry),
        )
    }

    fn waiting(&self) -> usize {
        self.waiting.load(Ordering::SeqCst)
    }
}

#[test]
fn carries_stay_in_order_across_a_newer_request() {
    let q = Queue::new();
    q.ask(carry(b"a"));
    q.ask(Request::Refresh { first: Vec::new() });
    q.ask(carry(b"b"));
    q.ask(carry(b"c"));
    q.ask(Request::Baseline);
    q.ask(carry(b"d"));
    assert_eq!(q.waiting(), 2);
    assert_eq!(q.next(), "carry a");
    // The refresh takes b and c aside, and stops at the baseline, which waits as the next.
    assert_eq!(q.next(), "refresh 0");
    assert_eq!(q.waiting(), 0);
    // Between the refresh's files: b first, while d waits in the queue behind the baseline.
    assert_eq!(q.carry_between_files(), "carry b");
    assert_eq!(q.next(), "baseline");
    // Between the baseline's runs: c, then d and e pulled after it.
    q.ask(carry(b"e"));
    assert_eq!(q.carry_between_files(), "carry c");
    assert_eq!(q.next(), "carry d");
    assert_eq!(q.next(), "carry e");
    assert_eq!(q.waiting(), 0);
}

#[test]
fn a_carry_is_never_counted_as_waiting() {
    let q = Queue::new();
    q.ask(carry(b"a"));
    assert_eq!(q.waiting(), 0, "sent");
    assert_eq!(q.carry_between_files(), "carry a");
    assert_eq!(q.waiting(), 0, "pulled");
    q.ask(carry(b"b"));
    assert_eq!(q.next(), "carry b");
    assert_eq!(q.waiting(), 0, "taken");
    q.ask(Request::Refresh { first: Vec::new() });
    q.ask(carry(b"c"));
    assert_eq!(q.waiting(), 1);
    assert_eq!(q.next(), "refresh 0");
    assert_eq!(q.waiting(), 0, "taken aside by the refresh");
    assert_eq!(q.next(), "carry c");
    assert_eq!(q.waiting(), 0);
    // A send to a thread that has ended counts nothing either.
    let (send, queue) = mpsc::channel();
    drop(queue);
    let waiting = AtomicUsize::new(0);
    assert!(!send_request(&send, &waiting, Request::Baseline));
    assert!(!send_request(&send, &waiting, carry(b"d")));
    assert_eq!(waiting.load(Ordering::SeqCst), 0);
}

#[test]
fn queued_carries_are_answered_as_failed_in_order_once_the_root_is_revoked() {
    let dir = scratch_dir("queue-revoked");
    let root = TrustedRoot::for_tests(dir.clone());
    let (sent, got) = mpsc::channel();
    let reviewer = Reviewer::start(root.clone(), None, move |report| {
        let asked = match report {
            Report::Failed { asked, .. } => Some(asked),
            _ => None,
        };
        sent.send(asked).is_ok()
    })
    .unwrap();
    root.revoke();
    // Whether the thread takes the refresh before `c` comes or with it aside, `c` is answered
    // after it; a request after `c` might go first, as a newer request does, so none is sent.
    for request in [
        carry(b"a"),
        carry(b"b"),
        Request::Refresh { first: Vec::new() },
        carry(b"c"),
    ] {
        assert!(reviewer.ask(request));
    }
    let answers: Vec<Option<Asked>> = (0..4)
        .map(|_| got.recv_timeout(Duration::from_secs(20)).unwrap())
        .collect();
    let carry = Some(Asked::Carry);
    assert_eq!(answers, [carry, carry, Some(Asked::Refresh), carry]);
    assert!(
        got.recv_timeout(Duration::from_millis(50)).is_err(),
        "no more"
    );
    drop(reviewer);
    std::fs::remove_dir_all(dir).unwrap();
}
