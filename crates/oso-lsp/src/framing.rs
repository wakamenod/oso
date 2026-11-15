//! Language server messages on a byte stream (ADR 0138): a header of CRLF lines with one
//! `Content-Length`, a blank line, and that many bytes. [`Reader`] reads them from a server's
//! standard output within the header's bounds, and reads a message over
//! [`MESSAGE_BYTES_MAX`] through in pieces and drops it, so the stream stays in step.
//! [`encode`] writes one.
//!
//! Must not depend on: `json`, `uri` or `position`; a frame's bytes are parsed elsewhere, so a
//! slow parse holds up no pipe.

use std::fmt;
use std::io::{self, Read};

use crate::limits::{
    CONTENT_LENGTH_DIGITS_MAX, DECLARED_BYTES_MAX, HEADER_BYTES_MAX, HEADER_LINE_BYTES_MAX,
    HEADER_LINES_MAX, MESSAGE_BYTES_MAX, READ_PIECE,
};

/// What one message on the stream was. Its `Debug` shows a length, never the bytes.
#[derive(Clone, PartialEq, Eq)]
pub enum Frame {
    /// A message within [`MESSAGE_BYTES_MAX`]: its bytes, not yet parsed.
    Message(Vec<u8>),
    /// A message over [`MESSAGE_BYTES_MAX`], read through and dropped unparsed.
    Dropped {
        /// Its length, from its header.
        len: u64,
    },
}

impl fmt::Debug for Frame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Message(body) => write!(f, "Message({} bytes)", body.len()),
            Self::Dropped { len } => write!(f, "Dropped({len} bytes)"),
        }
    }
}

/// Why a header is broken. Any of these stops the server (ADR 0138, Q12).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Broken {
    /// A line over [`HEADER_LINE_BYTES_MAX`].
    LineTooLong,
    /// More than [`HEADER_LINES_MAX`] lines.
    TooManyLines,
    /// More than [`HEADER_BYTES_MAX`] in all.
    TooLong,
    /// A line that does not end in CRLF, holds a CR or LF before it, or holds a byte that is
    /// not visible ASCII, a space or a tab.
    BadLine,
    /// A line with no `:` or an empty name.
    NoName,
    /// No `Content-Length` before the blank line.
    NoLength,
    /// `Content-Length` more than once.
    LengthTwice,
    /// A `Content-Length` that is not ASCII digits, has more than
    /// [`CONTENT_LENGTH_DIGITS_MAX`] of them, or is over [`DECLARED_BYTES_MAX`].
    BadLength,
}

impl fmt::Display for Broken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::LineTooLong => "a header line is too long",
            Self::TooManyLines => "a header has too many lines",
            Self::TooLong => "a header is too long",
            Self::BadLine => "a header line is not CRLF-ended visible ASCII",
            Self::NoName => "a header line has no name",
            Self::NoLength => "a header has no Content-Length",
            Self::LengthTwice => "a header has Content-Length twice",
            Self::BadLength => "a Content-Length is not a number",
        })
    }
}

/// Why no message could be read.
#[derive(Debug, PartialEq, Eq)]
pub enum FrameError {
    /// The header broke a rule. The stream is out of step from here.
    Broken(Broken),
    /// The stream ended inside a message or its header.
    Truncated,
    /// Reading failed.
    Io(io::ErrorKind),
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Broken(why) => why.fmt(f),
            Self::Truncated => f.write_str("the stream ended inside a message"),
            Self::Io(kind) => write!(f, "reading failed: {kind}"),
        }
    }
}

impl From<Broken> for FrameError {
    fn from(why: Broken) -> Self {
        Self::Broken(why)
    }
}

/// Reads messages from `R`, with a buffer of [`READ_PIECE`] bytes that holds what a read
/// brought past the current message.
pub struct Reader<R> {
    inner: R,
    buffer: Box<[u8]>,
    /// The unread bytes are `buffer[start..end]`.
    start: usize,
    end: usize,
}

impl<R: Read> Reader<R> {
    /// A reader of `inner`'s messages.
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            buffer: vec![0; READ_PIECE].into_boxed_slice(),
            start: 0,
            end: 0,
        }
    }

    /// The next message, or `None` if the stream ended cleanly between messages.
    ///
    /// # Errors
    ///
    /// A broken header, an end inside a message, or a failed read. After an error the
    /// stream is out of step, and the server is stopped.
    pub fn next_frame(&mut self) -> Result<Option<Frame>, FrameError> {
        let Some(len) = self.header()? else {
            return Ok(None);
        };
        match usize::try_from(len) {
            Ok(len) if len <= MESSAGE_BYTES_MAX => {
                self.body(len).map(|body| Some(Frame::Message(body)))
            }
            _ => self.skip(len).map(|()| Some(Frame::Dropped { len })),
        }
    }

    /// Reads a header and gives its `Content-Length`, or `None` at a clean end.
    fn header(&mut self) -> Result<Option<u64>, FrameError> {
        let mut length = None;
        let mut total = 0;
        for index in 0..HEADER_LINES_MAX {
            let Some(line) = self.line(index == 0)? else {
                return Ok(None);
            };
            total += line.len() + 2;
            if total > HEADER_BYTES_MAX {
                return Err(Broken::TooLong.into());
            }
            if line.is_empty() {
                return length.map(Some).ok_or(Broken::NoLength.into());
            }
            if let Some(found) = content_length(&line)?
                && length.replace(found).is_some()
            {
                return Err(Broken::LengthTwice.into());
            }
        }
        Err(Broken::TooManyLines.into())
    }

    /// The next header line without its CRLF, or `None` if the stream ended before a first
    /// line (`first`) began.
    fn line(&mut self, first: bool) -> Result<Option<Vec<u8>>, FrameError> {
        loop {
            let unread = &self.buffer[self.start..self.end];
            if let Some(newline) = unread.iter().position(|&byte| byte == b'\n') {
                if newline + 1 > HEADER_LINE_BYTES_MAX {
                    return Err(Broken::LineTooLong.into());
                }
                let line = &unread[..newline];
                let Some(text) = line.strip_suffix(b"\r") else {
                    return Err(Broken::BadLine.into());
                };
                if !text
                    .iter()
                    .all(|&byte| byte == b'\t' || (b' '..=b'~').contains(&byte))
                {
                    return Err(Broken::BadLine.into());
                }
                let text = text.to_vec();
                self.start += newline + 1;
                return Ok(Some(text));
            }
            if unread.len() >= HEADER_LINE_BYTES_MAX {
                return Err(Broken::LineTooLong.into());
            }
            if self.fill()? == 0 {
                return if first && self.start == self.end {
                    Ok(None)
                } else {
                    Err(FrameError::Truncated)
                };
            }
        }
    }

    /// Reads a body of `len` bytes: what the buffer holds of it, then the rest straight from
    /// the stream, so a body grows only as its bytes come.
    fn body(&mut self, len: usize) -> Result<Vec<u8>, FrameError> {
        let buffered = len.min(self.end - self.start);
        let mut body = Vec::with_capacity(buffered.max(len.min(READ_PIECE)));
        body.extend_from_slice(&self.buffer[self.start..self.start + buffered]);
        self.start += buffered;
        let left = (len - buffered) as u64;
        if left > 0 {
            (&mut self.inner)
                .take(left)
                .read_to_end(&mut body)
                .map_err(|error| FrameError::Io(error.kind()))?;
        }
        if body.len() < len {
            return Err(FrameError::Truncated);
        }
        Ok(body)
    }

    /// Reads `len` bytes through and drops them.
    fn skip(&mut self, len: u64) -> Result<(), FrameError> {
        let mut left = len;
        while left > 0 {
            if self.start == self.end && self.fill()? == 0 {
                return Err(FrameError::Truncated);
            }
            let unread = self.end - self.start;
            let take = usize::try_from(left).map_or(unread, |left| left.min(unread));
            self.start += take;
            left -= take as u64;
        }
        Ok(())
    }

    /// Moves the unread bytes to the buffer's start and reads more after them; the number
    /// read, 0 at the end of the stream.
    fn fill(&mut self) -> Result<usize, FrameError> {
        self.buffer.copy_within(self.start..self.end, 0);
        self.end -= self.start;
        self.start = 0;
        loop {
            match self.inner.read(&mut self.buffer[self.end..]) {
                Ok(read) => {
                    self.end += read;
                    return Ok(read);
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(FrameError::Io(error.kind())),
            }
        }
    }
}

/// The value of a `Content-Length` line, or `None` for another header, which is ignored.
fn content_length(line: &[u8]) -> Result<Option<u64>, Broken> {
    let Some(colon) = line.iter().position(|&byte| byte == b':') else {
        return Err(Broken::NoName);
    };
    let (name, value) = (&line[..colon], &line[colon + 1..]);
    if name.is_empty() {
        return Err(Broken::NoName);
    }
    if !name.eq_ignore_ascii_case(b"Content-Length") {
        return Ok(None);
    }
    let value = value.trim_ascii();
    if value.is_empty()
        || value.len() > CONTENT_LENGTH_DIGITS_MAX
        || !value.iter().all(u8::is_ascii_digit)
    {
        return Err(Broken::BadLength);
    }
    let length = value
        .iter()
        .fold(0, |sum, &digit| sum * 10 + u64::from(digit - b'0'));
    if length > DECLARED_BYTES_MAX {
        return Err(Broken::BadLength);
    }
    Ok(Some(length))
}

/// `body` as one message, in one buffer: its header, then its bytes.
pub fn encode(body: &[u8]) -> Vec<u8> {
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    let mut out = Vec::with_capacity(header.len() + body.len());
    out.extend_from_slice(header.as_bytes());
    out.extend_from_slice(body);
    out
}

#[cfg(test)]
mod tests;
