//! Tests of what the session thread counts and reports without a process: the crash policy,
//! the flood bound and the outbox's wakes and bounds. The life of a real process, against the
//! fake server, is tested in the tip crate's `lsp_server` tests, where the fake server is
//! built.
//!
//! Must not depend on: processes or pipes.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use super::State;
use super::outbox::{Outbox, Reporter};
use super::policy::{Crashes, Flood};
use crate::limits::{
    CRASH_WINDOW, FLOOD_BYTES_MAX, FLOOD_MESSAGES_MAX, REPORT_LOG_BYTES_MAX, RESTART_DELAYS,
    SHOW_MESSAGE_INTERVAL,
};
use crate::protocol::Severity;

#[test]
fn crashes_start_again_after_growing_waits_then_stop() {
    let at = Instant::now();
    let mut crashes = Crashes::default();
    assert_eq!(crashes.record(at), Some(RESTART_DELAYS[0]));
    assert_eq!(
        crashes.record(at + Duration::from_secs(10)),
        Some(RESTART_DELAYS[1])
    );
    assert_eq!(
        crashes.record(at + Duration::from_secs(60)),
        None,
        "a third"
    );
    crashes.forget();
    assert_eq!(
        crashes.record(at + Duration::from_secs(61)),
        Some(RESTART_DELAYS[0])
    );
}

#[test]
fn crashes_outside_the_window_are_forgotten() {
    let at = Instant::now();
    let mut crashes = Crashes::default();
    crashes.record(at);
    crashes.record(at + Duration::from_secs(60));
    let later = at + CRASH_WINDOW + Duration::from_secs(1);
    assert_eq!(
        crashes.record(later),
        Some(RESTART_DELAYS[1]),
        "two in the window"
    );
    assert_eq!(
        crashes.record(later + Duration::from_secs(1)),
        None,
        "three in the window"
    );
}

#[test]
fn a_flood_is_messages_or_bytes_within_the_window() {
    let at = Instant::now();
    let mut flood = Flood::new(at);
    for _ in 0..FLOOD_MESSAGES_MAX {
        assert!(!flood.count(at, 1));
    }
    assert!(flood.count(at + Duration::from_millis(900), 1), "one more");

    let mut flood = Flood::new(at);
    for _ in 0..FLOOD_MESSAGES_MAX {
        assert!(!flood.count(at, 1));
    }
    // Ten seconds on, the first second has left the window.
    assert!(!flood.count(at + Duration::from_secs(10), 1));

    let mut flood = Flood::new(at);
    assert!(!flood.count(at, FLOOD_BYTES_MAX as u64));
    assert!(flood.count(at + Duration::from_secs(9), 1), "a byte more");
    let mut flood = Flood::new(at);
    assert!(!flood.count(at, FLOOD_BYTES_MAX as u64));
    assert!(
        !flood.count(at + Duration::from_secs(10), 1),
        "the window moved on"
    );
}

#[test]
fn a_flood_spread_over_the_seconds_is_counted_in_all() {
    let at = Instant::now();
    let mut flood = Flood::new(at);
    let per_second = FLOOD_MESSAGES_MAX / 10;
    for second in 0..10 {
        for _ in 0..per_second {
            assert!(!flood.count(at + Duration::from_secs(second), 1));
        }
    }
    assert!(flood.count(at + Duration::from_secs(9), 1));
}

fn reporter() -> (Arc<Outbox>, Reporter, Arc<AtomicUsize>) {
    let outbox = Arc::new(Outbox::default());
    let wakes = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&wakes);
    let reporter = Reporter::new(
        Arc::clone(&outbox),
        Box::new(move || {
            counted.fetch_add(1, Ordering::SeqCst);
        }),
    );
    (outbox, reporter, wakes)
}

#[test]
fn a_start_wakes_at_once_and_progress_waits_for_the_interval() {
    let (outbox, mut reporter, wakes) = reporter();
    assert!(outbox.take().is_none(), "nothing yet");
    reporter.state(State::Ready);
    assert_eq!(wakes.load(Ordering::SeqCst), 1);
    reporter.state(State::Working(Some(10)));
    reporter.state(State::Working(Some(20)));
    assert_eq!(
        wakes.load(Ordering::SeqCst),
        1,
        "work and its percentage wait"
    );
    let now = Instant::now();
    assert!(reporter.flush(now).is_some(), "within the interval");
    assert_eq!(reporter.flush(now + Duration::from_millis(60)), None);
    assert_eq!(wakes.load(Ordering::SeqCst), 2);
    let report = outbox.take().unwrap();
    assert_eq!(report.state, State::Working(Some(20)));
    assert!(outbox.take().is_none(), "taken");
}

#[test]
fn the_log_keeps_its_last_bytes() {
    let (outbox, mut reporter, _) = reporter();
    let line = "x".repeat(1023);
    for _ in 0..(REPORT_LOG_BYTES_MAX / 1024 + 10) {
        reporter.log(line.clone());
    }
    let report = outbox.take().unwrap();
    assert_eq!(
        report.log[0],
        format!("(10 lines dropped, past {REPORT_LOG_BYTES_MAX} bytes)")
    );
    assert_eq!(report.log.len(), REPORT_LOG_BYTES_MAX / 1024 + 1);
}

#[test]
fn a_servers_messages_reach_the_person_once_a_second() {
    let (outbox, mut reporter, wakes) = reporter();
    let now = Instant::now();
    reporter.show(Severity::Info, "a".into(), true, now);
    reporter.show(
        Severity::Info,
        "b".into(),
        true,
        now + Duration::from_millis(500),
    );
    reporter.show(
        Severity::Error,
        "oso's own".into(),
        false,
        now + Duration::from_millis(600),
    );
    reporter.show(
        Severity::Info,
        "c".into(),
        true,
        now + SHOW_MESSAGE_INTERVAL,
    );
    let shown: Vec<String> = outbox
        .take()
        .unwrap()
        .shown
        .into_iter()
        .map(|s| s.text)
        .collect();
    assert_eq!(shown, ["a", "oso's own", "c"]);
    assert_eq!(wakes.load(Ordering::SeqCst), 3);
}

#[test]
fn work_beginning_and_ending_waits_for_the_interval() {
    let (_outbox, mut reporter, wakes) = reporter();
    reporter.state(State::Ready);
    let woken = wakes.load(Ordering::SeqCst);
    for _ in 0..1000 {
        reporter.state(State::Working(None));
        reporter.state(State::Ready);
    }
    assert_eq!(wakes.load(Ordering::SeqCst), woken, "none at once");
    reporter.state(State::Stopped);
    assert_eq!(wakes.load(Ordering::SeqCst), woken + 1, "a stop at once");
}

#[test]
fn empty_lines_of_standard_error_count_against_the_bound() {
    let kept = super::pipes::Stderr::default();
    for _ in 0..1_000_000 {
        kept.push_for_tests(b"");
    }
    let (lines, dropped) = kept.take();
    let cost = std::mem::size_of::<String>();
    assert!(
        lines.len() <= crate::limits::STDERR_KEPT_BYTES / cost,
        "{}",
        lines.len()
    );
    assert_eq!(lines.len() + dropped, 1_000_000);
}

#[test]
fn messages_for_the_person_are_kept_up_to_the_bound() {
    let (outbox, mut reporter, _) = reporter();
    let now = Instant::now();
    for n in 0..crate::limits::REPORT_SHOWN_MAX + 5 {
        reporter.show(Severity::Error, format!("m{n}"), false, now);
    }
    let shown = outbox.take().unwrap().shown;
    assert_eq!(shown.len(), crate::limits::REPORT_SHOWN_MAX);
    assert_eq!(shown[0].text, "m0");
}

#[test]
fn diagnostics_wait_as_the_latest_of_each_document_and_file_and_a_run_s_end_forgets_files() {
    use std::path::PathBuf;

    use super::Diagnosed;
    use crate::position::Encoding;
    use crate::protocol::Published;
    let (outbox, mut reporter, _) = reporter();
    // The version tells one report from another.
    let file = |name: &str, version: i32| Diagnosed::File {
        path: PathBuf::from(name),
        report: Arc::new(Published {
            uri: String::new(),
            version: Some(version),
            diagnostics: Vec::new(),
        }),
        encoding: Encoding::Utf16,
    };
    let open = |id| Diagnosed::Open {
        id,
        revision: 1,
        diagnostics: oso_core::diagnostic::Diagnostics::new("s", [], 0),
    };
    reporter.diagnostics(file("/w/a.rs", 1));
    reporter.diagnostics(open(4));
    reporter.diagnostics(file("/w/a.rs", 2));
    reporter.diagnostics(file("/w/b.rs", 1));
    let report = outbox.take().unwrap();
    assert_eq!(
        report.diagnostics,
        [open(4), file("/w/a.rs", 2), file("/w/b.rs", 1)]
    );
    reporter.diagnostics(file("/w/a.rs", 1));
    reporter.diagnostics(open(4));
    reporter.diagnostics(Diagnosed::Forgotten);
    reporter.diagnostics(file("/w/c.rs", 1));
    let report = outbox.take().unwrap();
    assert_eq!(
        report.diagnostics,
        [open(4), Diagnosed::Forgotten, file("/w/c.rs", 1)],
        "what came after the end stays"
    );
}
