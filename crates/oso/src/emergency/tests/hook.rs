//! The panic hook, what it does when writing or printing cannot get anywhere, and a panic
//! while publishing (ADR 0062). The hook itself is never installed here.
//!
//! Must not depend on: `app` or winit.

use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::{Scratch, published, scratch_editor, texts};
use crate::emergency::hook::{Place, on_panic, place_of, say_to};
use crate::emergency::{Limits, Why};

/// A writer that blocks until the test drops the sender it returns.
fn stuck() -> (
    std::sync::mpsc::Sender<()>,
    Mutex<std::sync::mpsc::Receiver<()>>,
) {
    let (release, stuck) = std::sync::mpsc::channel();
    (release, Mutex::new(stuck))
}

#[test]
fn a_write_that_stops_getting_anywhere_is_given_up_on() {
    let scratch = Scratch::new();
    let emergency = published(&scratch.copies(), &scratch_editor("stuck"));
    let started = Instant::now();
    let (release, stuck) = stuck();
    let limits = Limits {
        stall: Duration::from_millis(50),
        sync: Duration::from_secs(600),
    };
    let report = emergency.shared.write_copies_with(limits, move |_, _, _| {
        let _ = stuck.lock().unwrap().recv();
        Ok(())
    });
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!((report.wrote, report.failed), (0, 1));
    assert!(report.to_string().contains("stopped getting anywhere"));
    drop(release);
}

#[test]
fn a_slow_write_that_ticks_is_not_given_up_on() {
    let scratch = Scratch::new();
    let emergency = published(&scratch.copies(), &scratch_editor("slow"));
    // Longer than the limit in all, with far less than it between ticks.
    let limits = Limits {
        stall: Duration::from_secs(2),
        sync: Duration::from_secs(600),
    };
    let report = emergency.shared.write_copies_with(limits, |_, _, ticks| {
        for _ in 0..11 {
            std::thread::sleep(Duration::from_millis(200));
            ticks.progress();
        }
        Ok(())
    });
    assert_eq!(report.wrote, 1);
}

#[test]
fn a_long_sync_has_its_own_allowance_and_a_stuck_one_still_ends() {
    let scratch = Scratch::new();
    let emergency = published(&scratch.copies(), &scratch_editor("syncing"));
    let limits = Limits {
        stall: Duration::from_millis(50),
        sync: Duration::from_secs(600),
    };
    let report = emergency.shared.write_copies_with(limits, |_, _, ticks| {
        ticks.before_sync();
        std::thread::sleep(Duration::from_millis(300));
        ticks.progress();
        Ok(())
    });
    assert_eq!(
        report.wrote, 1,
        "past the stall limit, as a sync says nothing"
    );

    let emergency = published(&scratch.copies(), &scratch_editor("stuck syncing"));
    let (release, stuck) = stuck();
    let limits = Limits {
        stall: Duration::from_secs(600),
        sync: Duration::from_millis(50),
    };
    let report = emergency
        .shared
        .write_copies_with(limits, move |_, _, ticks| {
            ticks.before_sync();
            let _ = stuck.lock().unwrap().recv();
            Ok(())
        });
    assert_eq!(
        (report.wrote, report.failed),
        (0, 1),
        "a stuck sync does not hang"
    );
    drop(release);
}

#[test]
fn a_panic_while_publishing_neither_hangs_the_hook_nor_loses_what_it_can_still_use() {
    let scratch = Scratch::new();
    let emergency = published(&scratch.copies(), &scratch_editor("published"));

    // Held on this thread, as if the panic came with the lock taken. It cannot be read.
    let held = emergency.shared.state.lock().unwrap();
    let report = emergency.shared.write_copies(Why::Panic);
    assert_eq!(report.wrote, 0);
    assert!(
        report
            .to_string()
            .contains("while the text was being recorded")
    );
    drop(held);

    // A lock poisoned by that panic is as good as any: only moves happen under it.
    let shared = Arc::clone(&emergency.shared);
    let poisoned = std::thread::spawn(move || {
        let _held = shared.state.lock().unwrap();
        panic!("deliberate panic in a test, while publishing");
    })
    .join();
    assert!(poisoned.is_err());
    let report = emergency.shared.write_copies(Why::Panic);
    assert_eq!(report.wrote, 1, "the last published state");
    assert_eq!(texts(&scratch.copies()), ["published"]);
}

#[test]
fn the_hook_saves_only_outside_a_command_on_the_ui_thread() {
    let scratch = Scratch::new();
    let emergency = published(&scratch.copies(), &scratch_editor("text"));
    let shared = &emergency.shared;
    assert_eq!(place_of(shared), Place::OutsideCommand);

    let publisher = emergency.publisher();
    let boundary = publisher.enter_command();
    assert_eq!(place_of(shared), Place::InCommand);
    on_panic(shared, "here:1:1".to_owned(), io::sink());
    assert!(texts(&scratch.copies()).is_empty(), "the editor runs on");
    drop(boundary);
    assert_eq!(place_of(shared), Place::OutsideCommand);

    let other = Arc::clone(shared);
    std::thread::spawn(move || {
        assert_eq!(place_of(&other), Place::OtherThread);
        on_panic(&other, "here:1:1".to_owned(), io::sink());
    })
    .join()
    .unwrap();
    assert!(texts(&scratch.copies()).is_empty(), "it catches its own");

    on_panic(shared, "here:1:1".to_owned(), io::sink());
    assert_eq!(
        texts(&scratch.copies()),
        ["text"],
        "written before unwinding"
    );
}

/// A stderr nobody reads: a write never returns.
struct Blocked(Mutex<std::sync::mpsc::Receiver<()>>);

impl Write for Blocked {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        let _ = self.0.lock().unwrap().recv();
        Err(io::Error::other("the test is over"))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A stderr that has gone: every write fails.
struct Gone;

impl Write for Gone {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::other("the terminal has gone"))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_blocked_or_a_gone_stderr_holds_nothing_up() {
    let (release, blocked) = std::sync::mpsc::channel();
    let started = Instant::now();
    say_to(
        Blocked(Mutex::new(blocked)),
        || "words".to_owned(),
        Duration::from_millis(50),
    );
    assert!(started.elapsed() < Duration::from_secs(10), "it gave up");
    drop(release);
    say_to(Gone, || "words".to_owned(), Duration::from_secs(10));
}

#[test]
fn a_second_panic_under_one_command_writes_the_copies_as_the_process_is_going() {
    let scratch = Scratch::new();
    let emergency = published(&scratch.copies(), &scratch_editor("text"));
    let shared = &emergency.shared;
    let publisher = emergency.publisher();

    let boundary = publisher.enter_command();
    on_panic(shared, "here:1:1".to_owned(), io::sink());
    assert!(texts(&scratch.copies()).is_empty(), "caught");
    on_panic(shared, "here:2:2".to_owned(), io::sink());
    assert_eq!(
        texts(&scratch.copies()),
        ["text"],
        "a Drop during unwinding"
    );
    drop(boundary);

    // The next command starts afresh.
    let boundary = publisher.enter_command();
    assert_eq!(place_of(shared), Place::InCommand);
    assert!(
        !shared
            .panicked_in_command
            .load(std::sync::atomic::Ordering::Relaxed)
    );
    drop(boundary);
}
