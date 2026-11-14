//! A terminal's thread end to end, with `/bin/sh` in a real PTY: output reaches snapshots,
//! input reaches the program, a resize reaches it, the end carries the exit status, at most
//! one snapshot is in flight, and the input queue is bounded. Keys, pastes, focus, scrolls
//! and synchronized updates are in `input`.
//!
//! Must not depend on: the UI.

#![cfg(unix)]

mod input;

use std::ffi::OsString;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use oso_platform::process::Executable;

use super::{Input, QueueFull, Request, Start, Terminal};
use crate::engine::ghostty::tests::{palette, size};
use crate::event::{End, Event};
use crate::limits;
use crate::snapshot::Attrs;

pub(super) fn sh(script: &str) -> Start {
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
pub(super) struct Seen {
    pub terminal: Terminal,
    woken: mpsc::Receiver<()>,
    pub screen: Vec<String>,
    pub events: Vec<Event>,
    echo_off: bool,
    /// What each snapshot said of echo, in order: one `take` may drain several snapshots, and
    /// `echo_off` keeps only the last.
    echo_said: Vec<bool>,
    pub cursor: crate::snapshot::Cursor,
    size: (u16, u16),
    pub scrolled: usize,
    /// Snapshots taken.
    pub snapshots: usize,
}

impl Seen {
    pub fn start(start: Start) -> Seen {
        Seen::start_holding(start, limits::RENDER_HOLD_MAX)
    }

    /// A terminal whose synchronized updates hold snapshots for at most `hold_limit`.
    pub fn start_holding(start: Start, hold_limit: Duration) -> Seen {
        let (tx, woken) = mpsc::channel();
        let wake = Box::new(move || {
            let _ = tx.send(());
        });
        let terminal = Terminal::start_holding(start, wake, hold_limit).expect("start");
        Seen {
            terminal,
            woken,
            screen: Vec::new(),
            events: Vec::new(),
            echo_off: false,
            echo_said: Vec::new(),
            cursor: crate::snapshot::Cursor::default(),
            size: (0, 0),
            scrolled: 0,
            snapshots: 0,
        }
    }

    /// Takes snapshots until `done` holds, for at most 10 s.
    pub fn until(&mut self, done: impl Fn(&Seen) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done(self) {
            assert!(
                Instant::now() < deadline,
                "timed out: {:?} {:?} echo said {:?}",
                self.screen,
                self.events,
                self.echo_said
            );
            let _ = self.woken.recv_timeout(Duration::from_millis(50));
            self.take();
        }
    }

    /// Takes the snapshot that is ready, if one is.
    pub fn take(&mut self) {
        while let Some(mut snapshot) = self.terminal.take_snapshot() {
            self.snapshots += 1;
            self.scrolled = snapshot.scrolled;
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
            self.echo_said.push(snapshot.echo_off);
            self.cursor = snapshot.cursor;
            self.size = (snapshot.size.columns, snapshot.size.rows);
            self.events.append(&mut snapshot.events);
            self.terminal.recycle(snapshot);
        }
    }

    pub fn shows(&self, text: &str) -> bool {
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
fn empty_input_is_skipped_and_what_follows_still_arrives() {
    let mut seen = Seen::start(sh("read line; printf 'got %s\\n' \"$line\""));
    seen.terminal.send(Vec::new()).expect("nothing to queue");
    // An empty chunk that reaches the thread anyway is passed over, not waited on.
    seen.terminal
        .request(Request::Input(Input::Bytes(Vec::new())));
    seen.terminal.send(b"abc\r".to_vec()).expect("queued");
    seen.until(|s| s.shows("got abc"));
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
    seen.until(|s| s.size == (4, 1));
    seen.terminal.resize(size(5000, 5000));
    seen.until(|s| s.size == (1000, 500));
}

#[test]
fn echo_off_is_reported() {
    let mut seen = Seen::start(sh("stty -echo; printf 'ready\\n'; read x"));
    seen.until(|s| s.shows("ready") && s.echo_off);
}

#[test]
fn a_program_reading_keys_raw_is_not_a_password_prompt() {
    let mut seen = Seen::start(sh(
        "stty raw -echo; printf 'raw\\n'; read x; stty -raw; printf 'done\\n'; read y",
    ));
    seen.until(|s| s.shows("raw"));
    assert!(!seen.echo_off);
    // Raw, a carriage return is not a new line.
    seen.terminal.send(b"\n".to_vec()).expect("queued");
    seen.until(|s| s.shows("done"));
}

/// A file a script makes once a step of it is done, which a test waits for instead of timing
/// it: what the thread sees must not hang on how fast a runner forks.
struct Marker(std::path::PathBuf);

impl Marker {
    fn new(name: &str) -> Marker {
        let path = std::env::temp_dir().join(format!("oso-term-{}-{name}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        Marker(path)
    }

    /// The shell command that makes it.
    fn touch(&self) -> String {
        format!(": > '{}'", self.0.display())
    }

    fn wait(&self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !self.0.exists() {
            assert!(Instant::now() < deadline, "no {}", self.0.display());
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for Marker {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[test]
fn echo_turned_off_after_the_last_output_is_seen_at_the_next_key() {
    // Nothing is printed after `stty`: the look before the next key's write finds it, however
    // long the step took, and before the key goes.
    let off = Marker::new("off-after-output");
    let mut seen = Seen::start(sh(&format!(
        "printf 'ready\\n'; stty -echo; {}; read x",
        off.touch()
    )));
    seen.until(|s| s.shows("ready"));
    off.wait();
    seen.terminal.send(b"s".to_vec()).expect("queued");
    seen.until(|s| s.echo_off);
}

#[test]
fn a_synchronized_update_never_holds_a_password_prompt_back() {
    // The hold would last a minute: only the prompt itself lets the snapshot go.
    let off = Marker::new("held-off");
    let mut seen = Seen::start_holding(
        sh(&format!(
            "printf '\\033[?2026h'; stty -echo; {}; read x",
            off.touch()
        )),
        Duration::from_secs(60),
    );
    off.wait();
    // A key's look sees the prompt whenever `stty` ran.
    seen.terminal.send(b"s".to_vec()).expect("queued");
    seen.until(|s| s.echo_off);
}

#[test]
fn echo_back_on_with_no_output_is_seen() {
    let mut seen = Seen::start(sh(
        "stty -echo; printf 'ready\\n'; read x; stty echo; read y",
    ));
    seen.until(|s| s.shows("ready") && s.echo_off);
    seen.terminal.send(b"secret\r".to_vec()).expect("queued");
    seen.until(|s| !s.echo_off);
    assert!(!seen.shows("secret"), "{:?}", seen.screen);
    // A snapshot sent for the flags alone still says where the cursor is: below `ready`.
    assert_eq!((seen.cursor.row, seen.cursor.column), (1, 0));
}

#[test]
fn a_prompt_that_came_and_went_while_a_snapshot_waited_is_said_over_after() {
    let (off, on) = (Marker::new("came-off"), Marker::new("came-on"));
    let mut seen = Seen::start(sh(&format!(
        "read a; stty -echo; {}; read x; stty echo; {}; read y",
        off.touch(),
        on.touch()
    )));
    // The UI takes nothing while the prompt opens and ends: the first snapshot waits. It went
    // before `a`, so before the prompt could open.
    seen.woken
        .recv_timeout(Duration::from_secs(10))
        .expect("a snapshot");
    seen.terminal.send(b"a\r".to_vec()).expect("queued");
    off.wait();
    // The look before this write sees the prompt.
    seen.terminal.send(b"x\r".to_vec()).expect("queued");
    on.wait();
    // Time for the thread's last looks, so that it has seen the prompt end before the UI takes
    // anything: only the follow-up then says it is over. Passing never hangs on this.
    std::thread::sleep(Duration::from_millis(600));
    // The waiting snapshot says echo is on, the next the prompt, the one after that it is over:
    // each in its own snapshot, however many one `take` drains.
    seen.until(|s| s.echo_said.len() >= 3);
    assert_eq!(seen.echo_said[..3], [false, true, false]);
}

#[test]
fn the_pid_is_the_childs_while_it_runs_and_none_once_it_ended() {
    let mut seen = Seen::start(sh("printf 'pid %s\\n' $$; read line; exit 0"));
    seen.until(|s| s.shows("pid "));
    let shown = seen
        .screen
        .iter()
        .find_map(|line| line.strip_prefix("pid "))
        .and_then(|pid| pid.trim().parse::<u32>().ok());
    assert_eq!(seen.terminal.pid(), shown);
    seen.terminal.send(b"go\n".to_vec()).unwrap();
    seen.until(|s| s.end().is_some());
    assert_eq!(seen.terminal.pid(), None);
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
    let mut hold = super::thread::Hold::default();
    let now = Instant::now();
    assert!(super::thread::judge(
        &mut effects,
        &mut replies,
        &mut next,
        &mut hold,
        now
    ));
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
