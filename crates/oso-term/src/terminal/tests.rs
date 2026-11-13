//! A terminal's thread end to end, with `/bin/sh` in a real PTY: output reaches snapshots,
//! input reaches the program, a resize reaches it, the end carries the exit status, at most
//! one snapshot is in flight, and the input queue is bounded.
//!
//! Must not depend on: the UI.

#![cfg(unix)]

use std::ffi::OsString;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use oso_platform::process::Executable;

use super::{QueueFull, Start, Terminal};
use crate::engine::ghostty::tests::{palette, size};
use crate::event::{End, Event};
use crate::limits;
use crate::snapshot::Attrs;

fn sh(script: &str) -> Start {
    Start {
        executable: Executable::absolute("/bin/sh").expect("/bin/sh"),
        args: vec![OsString::from("-c"), OsString::from(script)],
        cwd: None,
        project: None,
        inherit: Vec::new(),
        set: Vec::new(),
        size: size(40, 6),
        scrollback: 100,
        palette: palette(),
    }
}

/// A started terminal, the screen as its snapshots built it, and what they reported.
struct Seen {
    terminal: Terminal,
    woken: mpsc::Receiver<()>,
    screen: Vec<String>,
    events: Vec<Event>,
    echo_off: bool,
    size: (u16, u16),
}

impl Seen {
    fn start(start: Start) -> Seen {
        let (tx, woken) = mpsc::channel();
        let terminal = Terminal::start(
            start,
            Box::new(move || {
                let _ = tx.send(());
            }),
        )
        .expect("start");
        Seen {
            terminal,
            woken,
            screen: Vec::new(),
            events: Vec::new(),
            echo_off: false,
            size: (0, 0),
        }
    }

    /// Takes snapshots until `done` holds, for at most 10 s.
    fn until(&mut self, done: impl Fn(&Seen) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done(self) {
            assert!(
                Instant::now() < deadline,
                "timed out: {:?} {:?}",
                self.screen,
                self.events
            );
            let _ = self.woken.recv_timeout(Duration::from_millis(50));
            while let Some(mut snapshot) = self.terminal.take_snapshot() {
                self.screen
                    .resize(usize::from(snapshot.size.rows), String::new());
                for row in &snapshot.rows {
                    let mut line = String::new();
                    for (column, cell) in row.cells.iter().enumerate() {
                        if cell.attrs.contains(Attrs::WIDE_SPACER) {
                            continue;
                        }
                        let cluster = row
                            .graphemes
                            .iter()
                            .find(|g| usize::from(g.0) == column)
                            .and_then(|(_, at)| row.text.get(at.start as usize..at.end as usize));
                        match cluster {
                            Some(text) => line.push_str(text),
                            None => line.push(cell.ch),
                        }
                    }
                    if let Some(at) = self.screen.get_mut(usize::from(row.index)) {
                        *at = line.trim_end().to_owned();
                    }
                }
                self.echo_off = snapshot.echo_off;
                self.size = (snapshot.size.columns, snapshot.size.rows);
                self.events.append(&mut snapshot.events);
                self.terminal.recycle(snapshot);
            }
        }
    }

    fn shows(&self, text: &str) -> bool {
        self.screen.iter().any(|line| line.contains(text))
    }

    fn end(&self) -> Option<&End> {
        self.events.iter().find_map(|e| match e {
            Event::Ended(end) => Some(end),
            _ => None,
        })
    }
}

#[test]
fn output_reaches_a_snapshot_and_the_end_carries_the_status() {
    let mut seen = Seen::start(sh("printf 'hello\\n'; exit 3"));
    seen.until(|s| s.end().is_some());
    assert!(seen.shows("hello"), "{:?}", seen.screen);
    match seen.end() {
        Some(End::Exited(status)) => assert_eq!(status.code(), Some(3)),
        other => panic!("{other:?}"),
    }
}

#[test]
fn clusters_keep_their_text_across_recycled_snapshots() {
    // e + U+0301, then u + U+0308, each a cluster of two code points, in two snapshots.
    let mut seen = Seen::start(sh(
        "printf 'e\\314\\201 one\\n'; read line; printf 'u\\314\\210 two\\n'",
    ));
    seen.until(|s| s.shows("one"));
    assert!(seen.shows("e\u{301} one"), "{:?}", seen.screen);
    seen.terminal.send(b"\r".to_vec()).expect("queued");
    seen.until(|s| s.shows("two"));
    assert!(seen.shows("u\u{308} two"), "{:?}", seen.screen);
    assert!(seen.shows("e\u{301} one"), "{:?}", seen.screen);
}

#[test]
fn input_reaches_the_program() {
    let mut seen = Seen::start(sh("read line; printf 'got %s\\n' \"$line\""));
    seen.terminal.send(b"abc\r".to_vec()).expect("queued");
    seen.until(|s| s.shows("got abc"));
}

#[test]
fn a_resize_reaches_the_program() {
    let mut seen = Seen::start(sh("read x; stty size"));
    seen.until(|s| !s.screen.is_empty());
    seen.terminal.resize(size(70, 9));
    seen.terminal.send(b"\r".to_vec()).expect("queued");
    seen.until(|s| s.shows("9 70"));
}

#[test]
fn a_size_is_clamped_to_the_limits() {
    let mut start = sh("read x");
    start.size = size(1, 0);
    let mut seen = Seen::start(start);
    seen.until(|s| s.size == (2, 1));
    seen.terminal.resize(size(5000, 5000));
    seen.until(|s| s.size == (1000, 500));
}

#[test]
fn echo_off_is_reported() {
    let mut seen = Seen::start(sh("stty -echo; printf 'ready\\n'; read x"));
    seen.until(|s| s.shows("ready") && s.echo_off);
}

#[test]
fn a_hang_up_ends_the_terminal() {
    let mut seen = Seen::start(sh("printf 'up\\n'; while :; do sleep 1; done"));
    seen.until(|s| s.shows("up"));
    seen.terminal.hang_up();
    seen.until(|s| s.end().is_some());
}

#[test]
fn at_most_one_snapshot_is_in_flight() {
    let wakes = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&wakes);
    let terminal = Terminal::start(
        sh("while :; do echo x; done"),
        Box::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        }),
    )
    .expect("start");
    let deadline = Instant::now() + Duration::from_secs(10);
    while wakes.load(Ordering::SeqCst) == 0 {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        wakes.load(Ordering::SeqCst),
        1,
        "a second snapshot came untaken"
    );
    assert!(terminal.take_snapshot().is_some());
    assert!(terminal.take_snapshot().is_none() || wakes.load(Ordering::SeqCst) > 1);
}

#[test]
fn the_input_queue_is_bounded() {
    let seen = Seen::start(sh("sleep 5"));
    let big = vec![b'x'; limits::INPUT_QUEUE_MAX + 1];
    assert_eq!(seen.terminal.send(big), Err(QueueFull));
    assert_eq!(seen.terminal.send(vec![b'x'; 10]), Ok(()));
}

#[test]
fn a_child_that_cannot_start_ends_the_terminal() {
    let mut start = sh("true");
    start.cwd = Some("/nonexistent-directory-for-oso-term".into());
    let mut seen = Seen::start(start);
    seen.until(|s| s.end().is_some());
    assert!(
        matches!(seen.end(), Some(End::Failed(_))),
        "{:?}",
        seen.events
    );
}

#[test]
fn a_snapshot_keeps_the_latest_state_and_at_most_256_other_events() {
    use crate::engine::Effect;
    use crate::snapshot::Snapshot;
    let mut effects: Vec<Effect> = (0..1000)
        .flat_map(|i| {
            [
                Effect::Title(format!("t{i}").into_bytes()),
                Effect::Notification {
                    title: Vec::new(),
                    body: b"n".to_vec(),
                },
                Effect::ClipboardWrite(vec![b'c'; 1000]),
            ]
        })
        .collect();
    let mut replies = Vec::new();
    let mut next = Snapshot::new(size(10, 3));
    assert!(super::thread::judge(&mut effects, &mut replies, &mut next));
    let titles: Vec<&str> = next
        .events
        .iter()
        .filter_map(|e| match e {
            Event::Title(t) => Some(t.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(titles, ["t999"]);
    let clips = next
        .events
        .iter()
        .filter(|e| matches!(e, Event::ClipboardWrite(_)))
        .count();
    assert_eq!(clips, 1);
    assert_eq!(
        next.events.len(),
        2 + 256 - 2,
        "the two state events count among the 256"
    );
    assert_eq!(next.dropped_events, 1000 - 254);
}
