//! A terminal over a restart (ADR 0120, step 11; ADR 0133), with `/bin/sh` in a real PTY: an
//! export stops reading and the next terminal reads what came meanwhile, nothing twice; a
//! full-screen program's screen comes across; a resume reads again; a state too large,
//! an abandoned or a wrong master end as they should; a release hangs nothing up.
//!
//! Must not depend on: the UI.

use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use super::{Seen, sh};
use crate::engine::ghostty::tests::palette;
use crate::event::End;
use crate::terminal::{Adopt, Encoding, Exported, NotExported, Terminal};

const BUDGET: usize = 64 << 20;

/// What `seen`'s terminal exports for `library`.
fn export(seen: &mut Seen, library: &str, budget: usize) -> Result<Exported, NotExported> {
    let answer = seen
        .terminal
        .export(library, Arc::new(AtomicUsize::new(budget)));
    let got = answer
        .recv_timeout(Duration::from_secs(10))
        .expect("an answer");
    seen.take();
    got
}

/// The terminal adopted from `exported`, its master not attached yet.
fn adopt(exported: &Exported) -> Seen {
    let (tx, woken) = mpsc::channel();
    let wake = Box::new(move || {
        let _ = tx.send(());
    });
    let adopt = Adopt {
        state: exported.state.clone(),
        encoding: exported.encoding,
        size: exported.size,
        scrollback: 100,
        palette: palette(),
        pid: exported.pid,
        started: exported.started,
    };
    let terminal = Terminal::adopt(adopt, wake).expect("adopted");
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

/// Hands `old`'s terminal over to a new one, as a restart does, and returns it reading.
fn hand_over(mut old: Seen, library: &str) -> Seen {
    let exported = export(&mut old, library, BUDGET).expect("exported");
    let mut new = adopt(&exported);
    old.terminal.release();
    new.terminal.attach(exported.master);
    new.until(|s| !s.screen.is_empty());
    new
}

#[test]
fn the_next_terminal_shows_the_screen_at_once_and_reads_what_came_meanwhile() {
    let mut old = Seen::start(sh(
        "printf 'one\\n'; read x; printf 'two %s\\n' \"$x\"; read y",
    ));
    old.until(|s| s.shows("one"));
    let pid = old.terminal.pid();
    let exported = export(&mut old, &Terminal::library(), BUDGET).expect("exported");
    // Carried to the same library build, the terminal travels as its snapshot.
    assert_eq!(exported.encoding, Encoding::Snapshot);
    assert_eq!(exported.pid, pid);
    assert!(exported.started.is_some(), "its start time is read");
    // Typed into the old one after the export: it still reaches the program, whose answer
    // waits in the kernel.
    old.terminal.send(b"abc\r".to_vec()).expect("queued");
    std::thread::sleep(Duration::from_millis(300));
    old.take();
    assert!(
        !old.shows("two"),
        "nothing is read after the export: {:?}",
        old.screen
    );
    let mut new = adopt(&exported);
    new.until(|s| s.shows("one"));
    assert_eq!(new.terminal.pid(), pid, "the child is the same");
    // Typed into the new one before its master comes: it waits.
    new.terminal.send(b"end\r".to_vec()).expect("queued");
    old.terminal.release();
    new.terminal.attach(exported.master);
    new.until(|s| s.shows("two abc"));
    assert_eq!(
        new.screen.iter().filter(|l| l.contains("two")).count(),
        1,
        "nothing twice: {:?}",
        new.screen
    );
    // `read y` took "end": the shell ends, seen from the PTY's end, with no status.
    new.until(|s| s.end().is_some());
    assert!(matches!(new.end(), Some(End::Closed)), "{:?}", new.end());
    assert_eq!(new.terminal.pid(), None);
}

#[test]
fn a_full_screen_programs_screen_comes_across_and_it_reads_on() {
    let mut old = Seen::start(sh("printf 'primary\\n\\033[?1049h\\033[Halt\\n'; read x; \
         printf '\\033[?1049lback\\n'; read y"));
    old.until(|s| s.shows("alt"));
    std::thread::sleep(Duration::from_millis(100));
    old.take();
    let mut new = hand_over(old, &Terminal::library());
    new.until(|s| s.shows("alt"));
    new.terminal.send(b"\r".to_vec()).expect("queued");
    new.until(|s| s.shows("back"));
    new.terminal.hang_up();
}

#[test]
fn another_library_gets_escape_sequences() {
    let mut old = Seen::start(sh("printf 'drawn\\n'; read x; printf 'later\\n'; read y"));
    old.until(|s| s.shows("drawn"));
    let exported = export(&mut old, "another library", BUDGET).expect("exported");
    assert_eq!(exported.encoding, Encoding::Escapes);
    let mut new = adopt(&exported);
    new.until(|s| s.shows("drawn"));
    old.terminal.release();
    new.terminal.attach(exported.master);
    new.terminal.send(b"\r".to_vec()).expect("queued");
    new.until(|s| s.shows("later"));
    new.terminal.hang_up();
}

#[test]
fn a_resume_reads_again_and_is_counted() {
    let mut seen = Seen::start(sh("printf 'one\\n'; read x; printf 'two\\n'; read y"));
    seen.until(|s| s.shows("one"));
    let exported = export(&mut seen, &Terminal::library(), BUDGET).expect("exported");
    drop(exported);
    seen.terminal.send(b"\r".to_vec()).expect("queued");
    assert_eq!(seen.terminal.resumes(), 0);
    seen.terminal.resume();
    seen.until(|s| s.shows("two"));
    assert_eq!(seen.terminal.resumes(), 1);
    seen.terminal.hang_up();
    seen.until(|s| s.end().is_some());
}

#[test]
fn a_state_too_large_is_refused_and_the_terminal_reads_on() {
    let mut seen = Seen::start(sh("printf 'one\\n'; read x; printf 'two\\n'; read y"));
    seen.until(|s| s.shows("one"));
    let refused = export(&mut seen, &Terminal::library(), 16);
    assert!(matches!(refused, Err(NotExported::TooLarge)), "{refused:?}");
    seen.terminal.send(b"\r".to_vec()).expect("queued");
    seen.until(|s| s.shows("two"));
    seen.terminal.hang_up();
}

#[test]
fn a_snapshot_carries_the_screen_not_in_use() {
    // Only the snapshot carries a screen the program is not showing (escape sequences carry the
    // one in use). A program draws on the primary screen, then on the alternate; carried as the
    // snapshot and leaving the alternate, it finds the primary screen as it was.
    let mut old = Seen::start(sh(
        "printf 'kept-primary\\n\\033[?1049h\\033[Hon-the-alt\\n'; read x; \
         printf '\\033[?1049l'; read y",
    ));
    old.until(|s| s.shows("on-the-alt"));
    std::thread::sleep(Duration::from_millis(100));
    old.take();
    let exported = export(&mut old, &Terminal::library(), BUDGET).expect("exported");
    assert_eq!(exported.encoding, Encoding::Snapshot);
    let mut new = adopt(&exported);
    new.until(|s| s.shows("on-the-alt"));
    old.terminal.release();
    new.terminal.attach(exported.master);
    // Leaving the alternate screen restores the primary, which only the snapshot carried.
    new.terminal.send(b"\r".to_vec()).expect("queued");
    new.until(|s| s.shows("kept-primary"));
    new.terminal.hang_up();
}

#[test]
fn an_abandoned_or_wrongly_attached_terminal_ends_failed() {
    let mut old = Seen::start(sh("printf 'one\\n'; read x"));
    old.until(|s| s.shows("one"));
    let exported = export(&mut old, &Terminal::library(), BUDGET).expect("exported");
    let mut abandoned = adopt(&exported);
    abandoned.terminal.abandon();
    abandoned.until(|s| s.end().is_some());
    assert!(
        matches!(abandoned.end(), Some(End::Failed(_))),
        "{:?}",
        abandoned.end()
    );
    let mut wrong = adopt(&exported);
    let (reader, _writer) = std::io::pipe().expect("a pipe");
    wrong.terminal.attach(reader.into());
    wrong.until(|s| s.end().is_some());
    assert!(
        matches!(wrong.end(), Some(End::Failed(_))),
        "{:?}",
        wrong.end()
    );
    // The old one still has its terminal.
    old.terminal.resume();
    old.terminal.hang_up();
    old.until(|s| s.end().is_some());
}

#[test]
fn a_released_terminal_hangs_nothing_up() {
    let mut old = Seen::start(sh(
        "printf 'one\\n'; read x; printf 'still %s\\n' \"$x\"; read y",
    ));
    old.until(|s| s.shows("one"));
    let mut new = hand_over(old, &Terminal::library());
    // Had the release hung up, the shell would have ended.
    std::thread::sleep(Duration::from_millis(200));
    new.terminal.send(b"here\r".to_vec()).expect("queued");
    new.until(|s| s.shows("still here"));
    new.terminal.hang_up();
    new.until(|s| s.end().is_some());
}

#[test]
fn terminals_share_one_budget_and_a_refusal_gives_back_what_it_took() {
    let mut first = Seen::start(sh("printf 'one\\n'; read x"));
    let mut second = Seen::start(sh("printf 'two\\n'; read x"));
    first.until(|s| s.shows("one"));
    second.until(|s| s.shows("two"));
    // Room for one terminal's state, not two.
    let size = export(&mut first, &Terminal::library(), BUDGET)
        .expect("exported")
        .state
        .len();
    first.terminal.resume();
    let budget = Arc::new(AtomicUsize::new(size + size / 2));
    let got = |seen: &mut Seen| {
        let answer = seen
            .terminal
            .export(&Terminal::library(), Arc::clone(&budget));
        answer
            .recv_timeout(Duration::from_secs(10))
            .expect("an answer")
    };
    assert!(got(&mut first).is_ok());
    let left = budget.load(std::sync::atomic::Ordering::Acquire);
    assert!(matches!(got(&mut second), Err(NotExported::TooLarge)));
    assert_eq!(
        budget.load(std::sync::atomic::Ordering::Acquire),
        left,
        "the refused one gave back what it took"
    );
    first.terminal.hang_up();
    second.terminal.hang_up();
}

#[test]
fn a_password_prompt_opened_while_exported_is_seen() {
    let mut seen = Seen::start(sh("printf 'one\\n'; read x; stty -echo; read y"));
    seen.until(|s| s.shows("one"));
    let exported = export(&mut seen, &Terminal::library(), BUDGET).expect("exported");
    drop(exported);
    seen.terminal.send(b"\r".to_vec()).expect("queued");
    seen.until(|s| s.echo_off);
    seen.terminal.resume();
    seen.terminal.hang_up();
}
