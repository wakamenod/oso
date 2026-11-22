//! A native session's stream read in lines and handed on in batches (ADR 0165).
//!
//! Must not depend on: a process; these tests read from bytes in memory.

use std::io::BufReader;
use std::sync::Mutex;
use std::sync::mpsc;

use super::{Item, MAX_WAITING, NotSent, Out, batch, next_line, offer, write};
use crate::sessions::native::wire::{Event, MAX_LINE};

fn lines(bytes: &[u8], capacity: usize) -> Vec<Result<Vec<u8>, usize>> {
    let mut reader = BufReader::with_capacity(capacity, bytes);
    let mut line = Vec::new();
    let mut read = Vec::new();
    while let Some(long) = next_line(&mut reader, &mut line).unwrap() {
        read.push(long.map_or_else(|| Ok(line.clone()), Err));
    }
    read
}

#[test]
fn lines_are_read_whole_across_reads() {
    for capacity in [1, 3, 64] {
        assert_eq!(
            lines(b"{\"a\":1}\n\n{\"b\"", capacity),
            vec![
                Ok(b"{\"a\":1}".to_vec()),
                Ok(Vec::new()),
                Ok(b"{\"b\"".to_vec())
            ]
        );
    }
}

#[test]
fn a_line_past_the_limit_is_skipped_and_reading_goes_on() {
    let mut bytes = vec![b'x'; MAX_LINE + 1];
    bytes.extend_from_slice(b"\nok\n");
    assert_eq!(
        lines(&bytes, 64 << 10),
        vec![Err(MAX_LINE + 1), Ok(b"ok".to_vec())]
    );
}

#[test]
fn events_go_on_in_batches_then_the_end() {
    let (queue, events) = mpsc::sync_channel(16);
    for _ in 0..3 {
        queue.send(Item::Event(Event::Other("x".into()))).unwrap();
    }
    queue
        .send(Item::Ended {
            status: Some(0),
            stderr: "bye".into(),
        })
        .unwrap();
    let got = std::sync::Arc::new(Mutex::new(Vec::new()));
    let into = std::sync::Arc::clone(&got);
    batch(&events, Box::new(move |out| into.lock().unwrap().push(out)));
    let got = std::mem::take(&mut *got.lock().unwrap());
    let events: usize = got
        .iter()
        .map(|out| match out {
            Out::Events(events) => events.len(),
            Out::Ended { .. } => 0,
        })
        .sum();
    assert_eq!(events, 3);
    assert!(matches!(
        got.last(),
        Some(Out::Ended { status: Some(0), stderr }) if stderr == "bye"
    ));
}

#[test]
fn a_reader_gone_without_an_end_ends_the_session() {
    let (queue, events) = mpsc::sync_channel(16);
    queue.send(Item::Event(Event::Other("x".into()))).unwrap();
    drop(queue);
    let got = std::sync::Arc::new(Mutex::new(Vec::new()));
    let into = std::sync::Arc::clone(&got);
    batch(&events, Box::new(move |out| into.lock().unwrap().push(out)));
    let got = std::mem::take(&mut *got.lock().unwrap());
    assert!(matches!(got.first(), Some(Out::Events(events)) if events.len() == 1));
    assert!(matches!(got.last(), Some(Out::Ended { status: None, .. })));
}

#[test]
fn a_batch_holds_at_most_its_most() {
    let (queue, events) = mpsc::sync_channel(super::MAX_BATCH * 3);
    for _ in 0..super::MAX_BATCH * 2 + 1 {
        queue.send(Item::Event(Event::Other("x".into()))).unwrap();
    }
    queue
        .send(Item::Ended {
            status: Some(0),
            stderr: String::new(),
        })
        .unwrap();
    let got = std::sync::Arc::new(Mutex::new(Vec::new()));
    let into = std::sync::Arc::clone(&got);
    batch(&events, Box::new(move |out| into.lock().unwrap().push(out)));
    let got = std::mem::take(&mut *got.lock().unwrap());
    let sizes: Vec<usize> = got
        .iter()
        .filter_map(|out| match out {
            Out::Events(events) => Some(events.len()),
            Out::Ended { .. } => None,
        })
        .collect();
    assert!(
        sizes.iter().all(|size| *size <= super::MAX_BATCH),
        "{sizes:?}"
    );
    assert_eq!(sizes.iter().sum::<usize>(), super::MAX_BATCH * 2 + 1);
}

/// What a writer wrote, shared with the test.
#[derive(Clone, Default)]
struct Written(std::sync::Arc<Mutex<Vec<u8>>>, bool);

impl std::io::Write for Written {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.1 {
            return Err(std::io::ErrorKind::BrokenPipe.into());
        }
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_send_never_waits_and_a_full_or_closed_input_refuses_it() {
    let (input, lines) = mpsc::sync_channel(MAX_WAITING);
    for n in 0..MAX_WAITING {
        assert_eq!(offer(&input, format!("{n}\n")), Ok(()));
    }
    assert_eq!(offer(&input, "late\n".into()), Err(NotSent::Full));
    // The writer writes them in order, and stops once the queue goes.
    let written = Written::default();
    let (kept, writer) = (
        written.clone(),
        std::thread::spawn(move || write(written, &lines)),
    );
    drop(input);
    writer.join().unwrap();
    let text = String::from_utf8(kept.0.lock().unwrap().clone()).unwrap();
    assert_eq!(text.lines().count(), MAX_WAITING);
    assert!(text.starts_with("0\n1\n"));
    let (input, lines) = mpsc::sync_channel::<String>(MAX_WAITING);
    drop(lines);
    assert_eq!(offer(&input, "x\n".into()), Err(NotSent::Closed));
}

#[test]
fn a_writer_whose_process_went_stops() {
    let (input, lines) = mpsc::sync_channel(MAX_WAITING);
    offer(&input, "a\n".into()).unwrap();
    offer(&input, "b\n".into()).unwrap();
    // It returns at the first failed write, with the queue still open.
    write(Written(std::sync::Arc::default(), true), &lines);
    drop(input);
}
