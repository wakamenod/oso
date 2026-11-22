//! A native session's stream read in lines and handed on in batches (ADR 0165).
//!
//! Must not depend on: a process; these tests read from bytes in memory.

use std::io::BufReader;
use std::sync::Mutex;
use std::sync::mpsc;

use super::{Item, Out, batch, next_line};
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
