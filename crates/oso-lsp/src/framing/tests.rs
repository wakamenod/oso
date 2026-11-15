//! The framing's bounds, one test each at the bound and past it; a round trip through reads
//! of every size; and a fuzz target of arbitrary streams.
//!
//! Must not depend on: a process; streams are bytes in memory.
//!
//! The fuzz target runs in `just ci` with proptest's default cases. For a long run:
//! `PROPTEST_CASES=200000 cargo test -p oso-lsp --release framing::tests::fuzz`.

use std::io::{self, Read};

use proptest::prelude::*;

use super::{Broken, Frame, FrameError, Reader, encode};
use crate::limits::{
    DECLARED_BYTES_MAX, HEADER_BYTES_MAX, HEADER_LINE_BYTES_MAX, HEADER_LINES_MAX,
    MESSAGE_BYTES_MAX, READ_PIECE,
};

/// Bytes handed out at most `step` at a time, as a pipe may.
struct Trickle {
    data: Vec<u8>,
    at: usize,
    step: usize,
}

impl Read for Trickle {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let take = self.step.min(out.len()).min(self.data.len() - self.at);
        out[..take].copy_from_slice(&self.data[self.at..self.at + take]);
        self.at += take;
        Ok(take)
    }
}

fn reader(data: Vec<u8>, step: usize) -> Reader<Trickle> {
    Reader::new(Trickle { data, at: 0, step })
}

/// Every frame in `data`, and how the stream ended.
fn frames(data: &[u8], step: usize) -> (Vec<Frame>, Option<FrameError>) {
    let mut reader = reader(data.to_vec(), step);
    let mut found = Vec::new();
    loop {
        match reader.next_frame() {
            Ok(Some(frame)) => found.push(frame),
            Ok(None) => return (found, None),
            Err(error) => return (found, Some(error)),
        }
    }
}

fn first(data: &[u8]) -> Result<Option<Frame>, FrameError> {
    reader(data.to_vec(), usize::MAX).next_frame()
}

fn broken(data: &[u8]) -> Option<Broken> {
    match first(data) {
        Err(FrameError::Broken(why)) => Some(why),
        _ => None,
    }
}

#[test]
fn a_frame_shows_its_length_and_not_its_bytes() {
    assert_eq!(
        format!("{:?}", Frame::Message(b"secret".to_vec())),
        "Message(6 bytes)"
    );
    assert_eq!(
        format!("{:?}", Frame::Dropped { len: 9 }),
        "Dropped(9 bytes)"
    );
}

#[test]
fn a_message_reads_back() {
    let data = encode(br#"{"jsonrpc":"2.0"}"#);
    assert_eq!(data, b"Content-Length: 17\r\n\r\n{\"jsonrpc\":\"2.0\"}");
    assert_eq!(
        frames(&data, 1),
        (vec![Frame::Message(br#"{"jsonrpc":"2.0"}"#.to_vec())], None)
    );
}

#[test]
fn other_headers_and_any_case_are_read() {
    let data = b"content-type: application/vscode-jsonrpc; charset=utf-8\r\n\
                 CONTENT-LENGTH:\t2 \r\n\r\n{}";
    assert_eq!(first(data), Ok(Some(Frame::Message(b"{}".to_vec()))));
}

#[test]
fn an_empty_stream_ends_cleanly_and_a_cut_one_does_not() {
    assert_eq!(first(b""), Ok(None));
    assert_eq!(first(b"Content-Len"), Err(FrameError::Truncated));
    assert_eq!(
        first(b"Content-Length: 5\r\n\r\nab"),
        Err(FrameError::Truncated)
    );
    assert_eq!(first(b"Content-Length: 5\r\n"), Err(FrameError::Truncated));
}

#[test]
fn line_ends_are_crlf_only() {
    assert_eq!(broken(b"Content-Length: 2\n\r\n{}"), Some(Broken::BadLine));
    assert_eq!(broken(b"Content-Length: 2\r\n\n{}"), Some(Broken::BadLine));
    assert_eq!(
        broken(b"Content-Length: 2\r\r\n\r\n{}"),
        Some(Broken::BadLine)
    );
    assert_eq!(
        broken(b"Content-Length: 2\x00\r\n\r\n{}"),
        Some(Broken::BadLine)
    );
    assert_eq!(
        broken("X: é\r\nContent-Length: 2\r\n\r\n{}".as_bytes()),
        Some(Broken::BadLine)
    );
}

#[test]
fn content_length_is_once_and_digits() {
    assert_eq!(broken(b"\r\n{}"), Some(Broken::NoLength));
    assert_eq!(broken(b"Content-Type: x\r\n\r\n{}"), Some(Broken::NoLength));
    assert_eq!(
        broken(b"Content-Length: 2\r\nContent-Length: 2\r\n\r\n{}"),
        Some(Broken::LengthTwice)
    );
    for bad in ["", " ", "+2", "-2", "2 2", "0x2", "2.0"] {
        let data = format!("Content-Length:{bad}\r\n\r\n{{}}");
        assert_eq!(broken(data.as_bytes()), Some(Broken::BadLength), "{bad:?}");
    }
    assert_eq!(broken(b"Content-Length 2\r\n\r\n{}"), Some(Broken::NoName));
    assert_eq!(broken(b": 2\r\n\r\n{}"), Some(Broken::NoName));
}

#[test]
fn content_length_digits_are_bounded() {
    let longest = format!("Content-Length: {}\r\n\r\n", "0".repeat(19));
    assert_eq!(
        first(longest.as_bytes()),
        Ok(Some(Frame::Message(Vec::new())))
    );
    let over = format!("Content-Length: {}\r\n\r\n", "0".repeat(20));
    assert_eq!(broken(over.as_bytes()), Some(Broken::BadLength));
}

#[test]
fn a_declared_length_past_the_flood_bound_is_broken() {
    // At the bound, it is read through: the stream ends inside it.
    let at = format!("Content-Length: {DECLARED_BYTES_MAX}\r\n\r\n");
    assert_eq!(first(at.as_bytes()), Err(FrameError::Truncated));
    let past = format!("Content-Length: {}\r\n\r\n", DECLARED_BYTES_MAX + 1);
    assert_eq!(broken(past.as_bytes()), Some(Broken::BadLength));
    let largest = format!("Content-Length: {}\r\n\r\n", "9".repeat(19));
    assert_eq!(broken(largest.as_bytes()), Some(Broken::BadLength));
}

/// An endless stream of spaces after a header.
struct Endless {
    header: Vec<u8>,
    at: usize,
}

impl Read for Endless {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let left = &self.header[self.at.min(self.header.len())..];
        let take = left.len().min(out.len());
        out[..take].copy_from_slice(&left[..take]);
        out[take..].fill(b' ');
        self.at += out.len();
        Ok(out.len())
    }
}

#[test]
fn an_endless_message_past_the_bound_ends_at_its_header() {
    let header = format!("Content-Length: {}\r\n\r\n", "9".repeat(19)).into_bytes();
    let mut reader = Reader::new(Endless { header, at: 0 });
    assert_eq!(
        reader.next_frame(),
        Err(FrameError::Broken(Broken::BadLength))
    );
}

/// A header of `lines` lines in all, the blank one included, with a `Content-Length: 0`.
fn header_of(lines: usize, filler: usize) -> Vec<u8> {
    let mut data = b"Content-Length: 0\r\n".to_vec();
    for _ in 2..lines {
        data.extend(format!("X: {}\r\n", "y".repeat(filler)).bytes());
    }
    data.extend(b"\r\n");
    data
}

#[test]
fn header_lines_are_bounded() {
    assert_eq!(
        first(&header_of(HEADER_LINES_MAX, 0)),
        Ok(Some(Frame::Message(Vec::new())))
    );
    assert_eq!(
        broken(&header_of(HEADER_LINES_MAX + 1, 0)),
        Some(Broken::TooManyLines)
    );
}

#[test]
fn a_header_line_is_bounded() {
    // "X: " and CRLF around the filler.
    let fits = HEADER_LINE_BYTES_MAX - 5;
    assert_eq!(
        first(&header_of(3, fits)),
        Ok(Some(Frame::Message(Vec::new())))
    );
    assert_eq!(broken(&header_of(3, fits + 1)), Some(Broken::LineTooLong));
    // With no line end at all, the line is too long as soon as it passes the bound.
    let endless = vec![b'x'; HEADER_LINE_BYTES_MAX + 1];
    assert_eq!(broken(&endless), Some(Broken::LineTooLong));
}

#[test]
fn a_whole_header_is_bounded() {
    // Content-Length's 19 bytes and the blank line's 2, and lines of 1 KiB.
    let filler = HEADER_LINE_BYTES_MAX - 5;
    let mut data = b"Content-Length: 0\r\n".to_vec();
    let mut total = 21;
    while total + HEADER_LINE_BYTES_MAX <= HEADER_BYTES_MAX {
        data.extend(format!("X: {}\r\n", "y".repeat(filler)).bytes());
        total += HEADER_LINE_BYTES_MAX;
    }
    let last = HEADER_BYTES_MAX - total - 5;
    data.extend(format!("X: {}\r\n\r\n", "y".repeat(last)).bytes());
    assert_eq!(data.len(), HEADER_BYTES_MAX);
    assert_eq!(first(&data), Ok(Some(Frame::Message(Vec::new()))));
    let mut over = data[..data.len() - 4].to_vec();
    over.extend(b"y\r\n\r\n");
    assert_eq!(broken(&over), Some(Broken::TooLong));
}

#[test]
fn a_message_at_the_bound_is_kept_and_past_it_dropped_in_step() {
    let at = vec![b' '; MESSAGE_BYTES_MAX];
    let past = vec![b' '; MESSAGE_BYTES_MAX + 1];
    let mut data = encode(&at);
    data.extend(encode(&past));
    data.extend(encode(b"{}"));
    let (found, end) = frames(&data, READ_PIECE * 3 + 7);
    assert_eq!(end, None);
    assert_eq!(found.len(), 3);
    assert_eq!(found[0], Frame::Message(at));
    assert_eq!(
        found[1],
        Frame::Dropped {
            len: (MESSAGE_BYTES_MAX + 1) as u64
        }
    );
    assert_eq!(found[2], Frame::Message(b"{}".to_vec()));
}

#[test]
fn a_declared_length_allocates_only_what_arrives() {
    let mut reader = reader(b"Content-Length: 16777216\r\n\r\nab".to_vec(), usize::MAX);
    assert_eq!(reader.next_frame(), Err(FrameError::Truncated));
}

#[test]
fn read_errors_are_reported_and_interrupts_retried() {
    struct Failing(Vec<io::ErrorKind>);
    impl Read for Failing {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(self.0.pop().unwrap_or(io::ErrorKind::BrokenPipe).into())
        }
    }
    let mut reader = Reader::new(Failing(vec![io::ErrorKind::Interrupted]));
    assert_eq!(
        reader.next_frame(),
        Err(FrameError::Io(io::ErrorKind::BrokenPipe))
    );
}

/// A stream of well-formed messages, some over the bound in size only by their header.
fn bodies() -> impl Strategy<Value = Vec<Vec<u8>>> {
    prop::collection::vec(prop::collection::vec(any::<u8>(), 0..300), 0..8)
}

proptest! {
    #[test]
    fn messages_read_back_whatever_the_reads(bodies in bodies(), step in 1usize..80) {
        let data: Vec<u8> = bodies.iter().flat_map(|body| encode(body)).collect();
        let (found, end) = frames(&data, step);
        prop_assert_eq!(end, None);
        let expected: Vec<Frame> = bodies.into_iter().map(Frame::Message).collect();
        prop_assert_eq!(found, expected);
    }

    #[test]
    fn a_cut_stream_never_reads_a_message_it_does_not_hold(
        bodies in bodies(),
        cut in any::<prop::sample::Index>(),
        step in 1usize..80,
    ) {
        let data: Vec<u8> = bodies.iter().flat_map(|body| encode(body)).collect();
        let cut = cut.index(data.len() + 1);
        let (found, end) = frames(&data[..cut], step);
        prop_assert!(found.len() <= bodies.len());
        for (frame, body) in found.iter().zip(&bodies) {
            prop_assert_eq!(frame, &Frame::Message(body.clone()));
        }
        let whole = found.iter().map(|frame| match frame {
            Frame::Message(body) => encode(body).len(),
            Frame::Dropped { .. } => 0,
        }).sum::<usize>();
        prop_assert_eq!(end.is_none(), whole == cut);
    }

    /// Arbitrary bytes, and bytes shaped like headers: the reader never panics, never gives
    /// a message over the bound, and stops at the first error.
    #[test]
    fn fuzz(
        parts in prop::collection::vec(prop_oneof![
            prop::collection::vec(any::<u8>(), 0..64),
            "[A-Za-z-]{0,20}: ?[0-9]{0,22}\r\n".prop_map(String::into_bytes),
            Just(b"Content-Length: 3\r\n\r\n{}}".to_vec()),
            Just(b"\r\n".to_vec()),
            Just(b"\n".to_vec()),
            Just(b"\r".to_vec()),
        ], 0..20),
        step in 1usize..64,
    ) {
        let data = parts.concat();
        let mut reader = reader(data.clone(), step);
        let mut taken = 0;
        loop {
            match reader.next_frame() {
                Ok(Some(Frame::Message(body))) => {
                    prop_assert!(body.len() <= MESSAGE_BYTES_MAX);
                    taken += body.len();
                    prop_assert!(taken <= data.len());
                }
                Ok(Some(Frame::Dropped { .. })) => prop_assert!(false, "nothing fits past the bound"),
                Ok(None) | Err(_) => break,
            }
        }
    }
}
