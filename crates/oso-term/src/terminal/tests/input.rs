//! Input through a terminal's thread, with `/bin/sh` in a real PTY: keys encoded against the
//! program's modes, pastes in brackets or refused, focus reports, scrolls and how input ends
//! them, the input queue's holds, and synchronized updates (mode 2026) with their 1 s limit.
//!
//! Must not depend on: the UI.

use std::time::{Duration, Instant};

use super::{Seen, sh};
use crate::event::Event;
use crate::limits;
use crate::snapshot::Scroll;
use crate::terminal::thread::Hold;
use crate::terminal::{Input, QueueFull, Shared, Start};

/// A program that sets `modes`, says it is ready, and shows the next `n` bytes it reads in
/// hex, after `got`.
fn reads(modes: &str, n: usize) -> Start {
    sh(&format!(
        "stty raw -echo; printf '{modes}ready\\r\\n'; \
         v=$(head -c {n} | od -An -tx1 | tr -d ' \\n'); printf 'got %s\\r\\n' \"$v\""
    ))
}

fn ready(modes: &str, n: usize) -> Seen {
    let mut seen = Seen::start(reads(modes, n));
    seen.until(|s| s.shows("ready"));
    seen
}

#[track_caller]
fn got(seen: &mut Seen, hex: &str) {
    seen.until(|s| s.shows("got"));
    assert!(seen.shows(&format!("got {hex}")), "{:?}", seen.screen);
}

fn key(seen: &Seen, key: &str) {
    let press = key.parse().expect("a key");
    seen.terminal.send_key(press).expect("queued");
}

#[test]
fn keys_are_encoded_against_the_programs_modes() {
    let mut seen = ready("", 6);
    key(&seen, "<up>");
    key(&seen, "C-a");
    key(&seen, "M-b");
    got(&mut seen, "1b5b41011b62");
    // Once the program asks for application cursor keys, the arrows follow.
    let mut seen = ready("\\033[?1h", 3);
    key(&seen, "<up>");
    got(&mut seen, "1b4f41");
}

#[test]
fn a_paste_goes_in_brackets_while_the_program_asks() {
    let mut seen = ready("\\033[?2004h", 15);
    let text = "a\r\nb\x1b".to_owned();
    seen.terminal.paste(text, false).expect("queued");
    got(&mut seen, "1b5b3230307e610a621b5b3230317e");
}

#[test]
fn a_paste_of_lines_without_brackets_is_refused_unless_allowed() {
    let mut seen = ready("", 3);
    seen.terminal
        .paste("a\nb".to_owned(), false)
        .expect("queued");
    seen.until(|s| {
        s.events
            .iter()
            .any(|e| matches!(e, Event::PasteRefused { lines: 2 }))
    });
    seen.terminal
        .paste("c\nd".to_owned(), true)
        .expect("queued");
    got(&mut seen, "630d64");
}

#[test]
fn focus_reports_go_only_while_the_program_asks_and_on_a_change() {
    let mut seen = ready("", 1);
    seen.terminal.set_focused(true);
    seen.terminal.send(b"x".to_vec()).expect("queued");
    got(&mut seen, "78");
    let mut seen = ready("\\033[?1004h", 6);
    seen.terminal.set_focused(true);
    seen.terminal.set_focused(true);
    seen.terminal.set_focused(false);
    got(&mut seen, "1b5b491b5b4f");
}

#[test]
fn a_scroll_moves_the_view_until_input_comes() {
    let mut seen = Seen::start(sh("seq 1 60; read x"));
    seen.until(|s| s.shows("60"));
    seen.terminal.scroll(Scroll::Lines(-5));
    seen.until(|s| s.scrolled == 5);
    assert!(seen.shows("51") && !seen.shows("60"), "{:?}", seen.screen);
    seen.terminal.send(b"\r".to_vec()).expect("queued");
    seen.until(|s| s.scrolled == 0);
}

#[test]
fn keys_and_pastes_hold_their_bound_of_the_input_queue() {
    let key = Input::Key("a".parse().expect("a key"));
    assert_eq!(key.reserved(), limits::KEY_BYTES_MAX);
    let paste = Input::Paste {
        text: "abc".to_owned(),
        lines_allowed: false,
    };
    assert_eq!(paste.reserved(), 3 + 12);
    assert_eq!(Input::Focus(true).reserved(), 0);
    let shared = Shared::default();
    let max = limits::INPUT_QUEUE_MAX;
    assert_eq!(shared.reserve(max - limits::KEY_BYTES_MAX), Ok(()));
    assert_eq!(shared.reserve(limits::KEY_BYTES_MAX + 1), Err(QueueFull));
    assert_eq!(shared.reserve(limits::KEY_BYTES_MAX), Ok(()));
    shared.release(max);
    assert_eq!(shared.reserve(usize::MAX), Err(QueueFull));
    let seen = Seen::start(sh("sleep 5"));
    let big = "x".repeat(max - 11);
    assert_eq!(seen.terminal.paste(big, true), Err(QueueFull));
}

/// The timing rule: a hold holds snapshots until it ends or is 1 s old, a beginning while one
/// is open does not extend it, and one that begins after an end holds again.
#[test]
fn a_hold_holds_until_it_ends_or_its_limit() {
    let limit = limits::RENDER_HOLD_MAX;
    let t0 = Instant::now();
    let at = |ms| t0 + Duration::from_millis(ms);
    let mut hold = Hold::default();
    assert!(!hold.holds(t0, limit));
    hold.set(true, t0);
    assert!(hold.holds(at(999), limit));
    assert_eq!(hold.left(at(400), limit), Some(Duration::from_millis(600)));
    // Reset before the limit.
    hold.set(false, at(500));
    assert!(!hold.holds(at(500), limit));
    assert_eq!(hold.left(at(500), limit), None);
    // Past the limit.
    hold.set(true, at(600));
    assert!(hold.holds(at(1599), limit));
    assert!(!hold.holds(at(1600), limit));
    assert_eq!(hold.left(at(1600), limit), None);
    hold.set(true, at(1700));
    assert!(
        !hold.holds(at(1700), limit),
        "a beginning while open extends nothing"
    );
    // Set again after a timeout, once the program reset it.
    hold.set(false, at(2000));
    hold.set(true, at(2100));
    assert!(hold.holds(at(2200), limit));
    assert!(!hold.holds(at(3100), limit));
}

#[test]
fn a_synchronized_update_holds_snapshots_until_it_ends() {
    let mut seen = Seen::start(sh(
        "printf 'before\\n'; sleep 0.2; printf '\\033[?2026hheld'; read x; \
         printf '\\033[?2026l'; read x",
    ));
    seen.until(|s| s.shows("before"));
    std::thread::sleep(Duration::from_millis(300));
    seen.take();
    assert!(!seen.shows("held"), "{:?}", seen.screen);
    let before = seen.snapshots;
    seen.terminal.send(b"\r".to_vec()).expect("queued");
    seen.until(|s| s.shows("held"));
    assert_eq!(seen.snapshots, before + 1, "the end sends one snapshot");
}

#[test]
fn a_hold_left_open_still_shows_its_output() {
    let start = sh("printf '\\033[?2026hopen'; read x");
    let mut seen = Seen::start_holding(start, Duration::from_millis(100));
    seen.until(|s| s.shows("open"));
}
