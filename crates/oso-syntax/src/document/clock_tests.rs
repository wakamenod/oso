//! Tests of the time limits in the worker's time (ADR 0172): a thread the system holds back
//! passes them on the wall clock but not on its own, and a crafted text still fails on the
//! wall clock, which is never less than the thread's, and on a thread given a fifth of it.
//!
//! Must not depend on: the worker; these call a document directly.
//!
//! - The clocks here read the system's time since 1970, scaled: tests keep no state between
//!   reads.

use std::time::{Duration, Instant};

use oso_core::rope::Rope;

use super::highlight::{MIN_PIECE, PIECE};
use super::tests::LONG;
use super::{Document, Failure, Highlighted, Parsed, Spent, Stopwatch, wall};
use crate::language::Language;
use crate::queries::Queries;

/// The worker's slice (ADR 0111).
const SLICE: Duration = Duration::from_millis(20);

/// A thread that never runs, by its own clock: all the time the wall clock passes, the system
/// held it back.
fn still() -> Option<Duration> {
    Some(Duration::ZERO)
}

/// The system's time since 1970.
fn since_1970() -> Option<Duration> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
}

/// A clock that runs a thousand times faster than the wall clock, as no thread's does.
fn racing() -> Option<Duration> {
    // Seconds since 1970 times a thousand still fit a `Duration`.
    since_1970()?.checked_mul(1000)
}

/// A thread the system gives a fifth of the wall clock's time, as on a loaded machine.
fn fifth() -> Option<Duration> {
    Some(since_1970()? / 5)
}

/// A clock that runs backwards, as no thread's does.
fn backwards() -> Option<Duration> {
    Some(Duration::MAX - since_1970()?)
}

/// A text that takes a parse, and its highlighting, more than a few slices on any machine.
fn large() -> Rope {
    Rope::from("fn f(a: u32) -> u32 { a + 1 }\n".repeat(20_000).as_str())
}

#[test]
fn a_parse_held_back_past_its_limits_goes_on() {
    let limits = Duration::from_micros(300);
    let parse = |clock: fn() -> Option<Duration>| {
        let mut document = Document::with_long_lines(Language::Rust, large());
        document.limits = (limits, limits, limits);
        for _ in 0..100_000 {
            match document.parse(Duration::from_micros(100), clock) {
                Parsed::Paused => continue,
                done => return done,
            }
        }
        Parsed::Paused
    };
    // On the wall clock it passes its limit, as the parse before ADR 0172 did...
    assert_eq!(parse(wall), Parsed::Failed(Failure::TooSlow));
    // ...but the thread ran none of that time...
    assert_eq!(parse(still), Parsed::Done);
    // ...and a thread that ran a fifth of it still passes it, later.
    assert_eq!(parse(fifth), Parsed::Failed(Failure::TooSlow));
}

#[test]
fn highlighting_held_back_past_its_limits_goes_on() {
    let highlight = |clock: fn() -> Option<Duration>| {
        let mut document = Document::with_long_lines(Language::Rust, large());
        assert_eq!(document.parse(LONG, wall), Parsed::Done);
        // The total limit, passed at the first slice the text needs more than.
        document.limits.2 = Duration::from_micros(300);
        let mut queries = Queries::new();
        let mut calls = 0;
        loop {
            calls += 1;
            match document.highlight(&mut queries, &[], SLICE, clock) {
                Highlighted::Paused if calls < 100_000 => continue,
                done => return (done, calls),
            }
        }
    };
    let (done, calls) = highlight(wall);
    assert_eq!(
        done,
        Highlighted::Failed(Failure::TooSlow),
        "{calls} slices"
    );
    let (done, calls) = highlight(still);
    assert_eq!(done, Highlighted::Done);
    assert!(calls > 1, "the text needs more than a slice");
}

/// Highlights `text` in up to `slices` slices on `clock`, with the total limit `limit`: what it
/// came to, and how large pieces are then.
fn crafted(
    text: &str,
    slices: usize,
    limit: Duration,
    clock: fn() -> Option<Duration>,
) -> (Highlighted, usize) {
    let mut document = Document::with_long_lines(Language::Rust, Rope::from(text));
    document.limits.2 = limit;
    let mut queries = Queries::new();
    let done = (0..slices)
        .map(|_| document.highlight(&mut queries, &[], SLICE, clock))
        .find(|done| *done != Highlighted::Paused)
        .unwrap_or(Highlighted::Paused);
    (done, document.max_piece)
}

/// A crafted text no piece of which fits a slice: on the wall clock it fails in a few slices,
/// as `crafted` checks, but a thread held back all that time judges no piece by it. Pieces
/// still shrink, by the wall clock, which costs nothing and fits them to the time the thread
/// gets.
#[test]
fn a_piece_held_back_shrinks_but_is_not_too_slow() {
    let text = "(".repeat(64 << 10);
    let (done, _) = crafted(&text, 10, super::highlight::HIGHLIGHT_LIMIT, wall);
    assert_eq!(done, Highlighted::Failed(Failure::TooSlow));
    let (done, piece) = crafted(&text, 10, super::highlight::HIGHLIGHT_LIMIT, still);
    assert_eq!(done, Highlighted::Paused);
    assert_eq!(piece, MIN_PIECE);
}

/// The same text on a thread given a fifth of the machine still fails as too slow, once the
/// fifth reaches the limit: about five times later on the wall clock, which is the load's
/// cost. The limit is shortened, so that the test takes about half a second.
#[test]
fn crafted_text_on_a_loaded_machine_still_fails() {
    let text = "(".repeat(64 << 10);
    let (done, piece) = crafted(&text, 10_000, Duration::from_millis(100), fifth);
    assert_eq!(done, Highlighted::Failed(Failure::TooSlow));
    assert!(piece < PIECE, "{piece}");
}

#[test]
fn the_workers_time_is_never_more_than_the_wall_clocks() {
    let started = Instant::now();
    let watch = Stopwatch::start(racing);
    std::thread::sleep(Duration::from_millis(1));
    let took = watch.elapsed();
    assert!(took <= started.elapsed(), "{took:?}");
    assert!(took >= Duration::from_millis(1), "{took:?}");
}

#[test]
fn a_clock_that_reads_none_times_on_the_wall_clock() {
    let watch = Stopwatch::start(wall);
    std::thread::sleep(Duration::from_millis(1));
    assert!(watch.elapsed() >= Duration::from_millis(1));
}

#[test]
fn a_clock_that_runs_backwards_counts_nothing() {
    let watch = Stopwatch::start(backwards);
    std::thread::sleep(Duration::from_millis(1));
    assert_eq!(watch.elapsed(), Duration::ZERO);
    let mut spent = Spent::default();
    spent.add(&watch);
    assert_eq!(spent.total(), Duration::ZERO);
}

/// A limit counts the CPU time in all, never more than the wall clock's in all.
#[test]
fn time_spent_is_never_more_than_the_wall_clocks_in_all() {
    let started = Instant::now();
    let (mut racing_spent, mut wall_spent) = (Spent::default(), Spent::default());
    for _ in 0..2 {
        let (fast, slow) = (Stopwatch::start(racing), Stopwatch::start(wall));
        std::thread::sleep(Duration::from_millis(1));
        racing_spent.add(&fast);
        wall_spent.add(&slow);
    }
    assert!(racing_spent.total() <= started.elapsed());
    assert!(racing_spent.total() >= Duration::from_millis(2));
    assert!(wall_spent.total() >= Duration::from_millis(2));
}
