//! Tests of the clipboard's threads over the clipboard in memory and a slow backend:
//! its own text, text from another app, images, the bounds and the read's time limit.
//!
//! Must not depend on: the system's clipboard, which a test must never touch.

use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Instant;

use super::threads::{Factory, Timing};
use super::*;

const MAX: usize = 16;

fn memory() -> (Clipboard, Receiver<Event>) {
    let (tx, rx) = mpsc::channel();
    let clipboard = Clipboard::start(Source::Memory, MAX, move |event| {
        let _ = tx.send(event);
    })
    .unwrap();
    (clipboard, rx)
}

fn next(events: &Receiver<Event>) -> Event {
    events.recv_timeout(Duration::from_secs(5)).unwrap()
}

fn read(clipboard: &Clipboard, events: &Receiver<Event>) -> Contents {
    clipboard.read();
    match next(events) {
        Event::Read(contents) => contents,
        other => panic!("{other:?}"),
    }
}

fn read_image(clipboard: &Clipboard, events: &Receiver<Event>) -> ImageContents {
    clipboard.read_image();
    match next(events) {
        Event::Image(contents) => contents,
        other => panic!("{other:?}"),
    }
}

fn write(
    clipboard: &Clipboard,
    events: &Receiver<Event>,
    id: u64,
    text: &'static str,
) -> Fingerprint {
    clipboard.write(id, false, Box::new(move || text.to_owned()));
    match next(events) {
        Event::Written {
            id: written,
            fingerprint,
        } => {
            assert_eq!(written, id);
            fingerprint
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn its_own_text_reads_as_its_own() {
    let (clipboard, events) = memory();
    assert_eq!(read(&clipboard, &events), Contents::Nothing);
    let fingerprint = write(&clipboard, &events, 7, "killed");
    assert_eq!(fingerprint.len, 6);
    assert_eq!(read(&clipboard, &events), Contents::Own);
    assert_eq!(
        clipboard.memory().unwrap().get(),
        Some(("killed".into(), false))
    );
}

#[test]
fn text_from_another_app_comes_with_its_fingerprint() {
    let (clipboard, events) = memory();
    let written = write(&clipboard, &events, 1, "key");
    clipboard.memory().unwrap().set("key".into(), true);
    let Contents::Text {
        text,
        concealed,
        fingerprint,
    } = read(&clipboard, &events)
    else {
        panic!("not text");
    };
    assert_eq!((text.as_str(), concealed), ("key", true));
    assert_eq!(fingerprint, written, "the same text, the same fingerprint");

    clipboard.memory().unwrap().set("other".into(), false);
    let Contents::Text { fingerprint, .. } = read(&clipboard, &events) else {
        panic!("not text");
    };
    assert_ne!(fingerprint, written);
}

#[test]
fn text_over_the_bound_is_not_read() {
    let (clipboard, events) = memory();
    clipboard.memory().unwrap().set("x".repeat(MAX + 1), false);
    assert_eq!(read(&clipboard, &events), Contents::TooLarge);
}

#[test]
fn a_concealed_write_reaches_the_clipboard_marked() {
    let (clipboard, events) = memory();
    clipboard.write(1, true, Box::new(|| "secret".to_owned()));
    let _ = next(&events);
    assert_eq!(
        clipboard.memory().unwrap().get(),
        Some(("secret".into(), true))
    );
}

/// A clipboard over `backend` for its first worker, and one that holds nothing for any
/// worker after it.
fn over(
    backend: impl Backend + Send + 'static,
    timing: Timing,
    report: impl Fn(Event) + Send + 'static,
) -> Clipboard {
    let mut first: Option<Box<dyn Backend + Send>> = Some(Box::new(backend));
    let make: Factory = Box::new(move || first.take().unwrap_or_else(|| Box::new(Empty)));
    Clipboard::spawn(make, MAX, timing, report).unwrap()
}

/// A clipboard that holds nothing.
struct Empty;

impl Backend for Empty {
    fn change_count(&mut self) -> Option<i64> {
        None
    }

    fn write(&mut self, _: &str, _: bool) -> Option<i64> {
        None
    }

    fn read(&mut self, _: usize) -> Found {
        Found::Nothing
    }

    fn read_image(&mut self, _: usize) -> ImageContents {
        ImageContents::Nothing
    }
}

/// A backend whose reads take as long as a test says.
struct Slow(Receiver<()>);

impl Backend for Slow {
    fn change_count(&mut self) -> Option<i64> {
        None
    }

    fn write(&mut self, _: &str, _: bool) -> Option<i64> {
        None
    }

    fn read(&mut self, _: usize) -> Found {
        let _ = self.0.recv();
        Found::Nothing
    }

    fn read_image(&mut self, _: usize) -> ImageContents {
        let _ = self.0.recv();
        ImageContents::Nothing
    }
}

#[test]
fn a_read_that_takes_too_long_is_answered_in_time() {
    let (release, wait) = mpsc::channel();
    let (tx, events) = mpsc::channel();
    let clipboard = over(Slow(wait), TIMING, move |event| {
        let _ = tx.send(event);
    });
    let started = Instant::now();
    assert_eq!(read(&clipboard, &events), Contents::NoAnswer);
    let waited = started.elapsed();
    assert!(
        waited >= READ_TIMEOUT && waited < READ_TIMEOUT * 4,
        "{waited:?}"
    );

    // The backend is still on it, so the next read is answered at once.
    assert_eq!(read(&clipboard, &events), Contents::NoAnswer);
    // The late answer goes nowhere; once it is in, the next read gets its own.
    release.send(()).unwrap();
    release.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let contents = loop {
        match read(&clipboard, &events) {
            Contents::NoAnswer if Instant::now() < deadline => thread::yield_now(),
            contents => break contents,
        }
    };
    assert_eq!(contents, Contents::Nothing);
    assert!(events.try_recv().is_err(), "one answer for each read");
}

#[test]
fn dropping_the_handle_ends_both_threads() {
    let (clipboard, events) = memory();
    drop(clipboard);
    let ended = thread::spawn(move || events.recv()).join().unwrap();
    assert!(
        ended.is_err(),
        "the report closure was dropped with the threads"
    );
}

#[test]
fn debug_never_shows_the_text() {
    let contents = Contents::Text {
        text: "hunter2".into(),
        concealed: false,
        fingerprint: Fingerprint { len: 7, hash: 0 },
    };
    assert!(!format!("{contents:?}").contains("hunter2"));
}

/// A backend whose writes wait until a test lets them through, and which records them.
struct Gate {
    open: Receiver<()>,
    written: mpsc::Sender<String>,
}

impl Backend for Gate {
    fn change_count(&mut self) -> Option<i64> {
        None
    }

    fn write(&mut self, text: &str, _: bool) -> Option<i64> {
        let _ = self.open.recv();
        let _ = self.written.send(text.to_owned());
        None
    }

    fn read(&mut self, _: usize) -> Found {
        Found::Nothing
    }

    fn read_image(&mut self, _: usize) -> ImageContents {
        ImageContents::Nothing
    }
}

#[test]
fn a_newer_write_replaces_one_not_yet_started_and_a_read_stays_after_it() {
    let (open, gate) = mpsc::channel();
    let (written_tx, written) = mpsc::channel();
    let (tx, events) = mpsc::channel();
    let backend = Gate {
        open: gate,
        written: written_tx,
    };
    let clipboard = over(backend, TIMING, move |event| {
        let _ = tx.send(event);
    });
    clipboard.write(1, false, Box::new(|| "one".to_owned()));
    // Once the backend waits in the first write, these queue behind it.
    thread::sleep(Duration::from_millis(50));
    clipboard.write(2, false, Box::new(|| "two".to_owned()));
    clipboard.write(3, false, Box::new(|| "three".to_owned()));
    clipboard.read();
    thread::sleep(Duration::from_millis(50));
    open.send(()).unwrap();
    open.send(()).unwrap();
    let ids: Vec<u64> = (0..2)
        .map(|_| match next(&events) {
            Event::Written { id, .. } => id,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(ids, [1, 3], "the second never started");
    assert!(matches!(next(&events), Event::Read(Contents::Nothing)));
    assert_eq!(written.try_iter().collect::<Vec<_>>(), ["one", "three"]);
}

#[test]
fn a_read_that_never_returns_is_left_behind_and_a_new_worker_takes_over() {
    let (_never, wait) = mpsc::channel();
    let (tx, events) = mpsc::channel();
    let timing = Timing {
        answer: Duration::from_millis(50),
        give_up: Duration::from_millis(200),
    };
    let clipboard = over(Slow(wait), timing, move |event| {
        let _ = tx.send(event);
    });
    clipboard.read();
    assert!(matches!(next(&events), Event::Read(Contents::NoAnswer)));
    // A write while the read is out waits in the front, not behind the read.
    clipboard.write(7, false, Box::new(|| "after".to_owned()));
    assert!(matches!(next(&events), Event::NotAnswering));
    assert!(matches!(next(&events), Event::Written { id: 7, .. }));
    assert_eq!(
        read(&clipboard, &events),
        Contents::Nothing,
        "the new worker answers"
    );
}

#[test]
fn an_image_from_another_app_is_read_as_its_bytes() {
    let (clipboard, events) = memory();
    assert_eq!(read_image(&clipboard, &events), ImageContents::Nothing);
    let memory = clipboard.memory().unwrap();
    memory.set_image(ImageKind::Png, vec![1, 2, 3], false);
    assert_eq!(
        read_image(&clipboard, &events),
        ImageContents::Image(Image {
            kind: ImageKind::Png,
            bytes: vec![1, 2, 3]
        })
    );
    assert_eq!(
        read(&clipboard, &events),
        Contents::Nothing,
        "it holds no text"
    );
    // Text copied after it replaces it.
    memory.set("text".into(), false);
    assert_eq!(read_image(&clipboard, &events), ImageContents::Nothing);
}

#[test]
fn a_concealed_or_empty_image_is_not_read() {
    let (clipboard, events) = memory();
    let memory = clipboard.memory().unwrap();
    memory.set_image(ImageKind::Png, vec![1], true);
    assert_eq!(read_image(&clipboard, &events), ImageContents::Concealed);
    memory.set_image(ImageKind::Png, Vec::new(), false);
    assert_eq!(read_image(&clipboard, &events), ImageContents::Nothing);
}

#[test]
fn an_image_over_the_bound_is_not_read() {
    let (clipboard, events) = memory();
    let memory = clipboard.memory().unwrap();
    memory.set_image(ImageKind::Tiff, vec![0; MAX_IMAGE + 1], false);
    assert_eq!(read_image(&clipboard, &events), ImageContents::TooLarge);
    memory.set_image(ImageKind::Tiff, vec![0; MAX_IMAGE], false);
    assert!(matches!(
        read_image(&clipboard, &events),
        ImageContents::Image(Image {
            kind: ImageKind::Tiff,
            ..
        })
    ));
}

#[test]
fn an_image_read_waits_until_the_give_up_time_and_a_new_worker_takes_over() {
    let (_never, wait) = mpsc::channel();
    let (tx, events) = mpsc::channel();
    let timing = Timing {
        answer: Duration::from_millis(50),
        give_up: Duration::from_millis(300),
    };
    let clipboard = over(Slow(wait), timing, move |event| {
        let _ = tx.send(event);
    });
    let started = Instant::now();
    clipboard.read_image();
    // A read while it is out is answered at once, and a text read too.
    clipboard.read_image();
    assert!(matches!(
        next(&events),
        Event::Image(ImageContents::NoAnswer)
    ));
    clipboard.read();
    assert!(matches!(next(&events), Event::Read(Contents::NoAnswer)));
    assert!(started.elapsed() < timing.give_up, "both at once");
    // Its own answer comes at the give-up time, not the text read's answer time.
    assert!(matches!(
        next(&events),
        Event::Image(ImageContents::NoAnswer)
    ));
    assert!(started.elapsed() >= timing.give_up);
    assert_eq!(
        read_image(&clipboard, &events),
        ImageContents::Nothing,
        "the new worker answers"
    );
    assert!(events.try_recv().is_err(), "one answer for each read");
}

#[test]
fn debug_never_shows_an_image() {
    let image = Image {
        kind: ImageKind::Png,
        bytes: b"hunter2".to_vec(),
    };
    let shown = format!("{:?}", ImageContents::Image(image));
    assert!(!shown.contains("104"), "{shown}");
    assert!(shown.contains("len: 7"), "{shown}");
}
