//! Jumping from a review to the source (ADR 0122, step 3, part three), on the review's own
//! thread, as the author chose (Q5): where a line of the reviewed new side is in the file's text
//! now, carried through the edits made since the reading; and a side's text built for a
//! read-only buffer.
//!
//! Must not depend on: `app`, `dispatch` or git's runner; this works on bytes the host hands
//! in, and reads only the work tree's file, as a reading does (`side`).
//!
//! - A jump waits with the carries, answered between the files of a running request and when
//!   nothing runs, so a refresh never keeps it waiting long and it stops nothing.
//! - The text now is the open buffer's, handed in at O(1), or the work tree's file read from the
//!   top a directory at a time, following no link, and not read if it has another name; either
//!   is used only up to [`MAX_BLOB`], a side's limit.
//! - The new side and the text now are diffed by lines within [`DEADLINE`], each at most
//!   [`MAX_LINES`](super::diff::MAX_LINES) lines; past either the line is not carried, and the
//!   host says so and goes to the line as it was.
//! - A line changed since goes to the start of the span that replaced it (the Decision's "a line
//!   inside a changed span goes to the span's start").
//! - Nothing here logs a file's text.

use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use oso_core::display_map::TabWidth;
use oso_core::load::{Builder, Reloaded};
use oso_core::review::{BaseHunk, Hunk, Side};
use oso_core::rope::Rope;
use oso_platform::file_meta::Directory;

use super::side::{self, Found, Got};
use super::{MAX_BLOB, diff};

/// How long carrying a line through the edits since its reading may take.
pub(crate) const DEADLINE: Duration = Duration::from_secs(1);

/// A jump the host asks for, answered by its `ticket`.
pub(crate) struct Jump {
    pub(crate) ticket: u64,
    pub(crate) ask: Ask,
}

/// What a jump asks for.
pub(crate) enum Ask {
    /// Where line `line` of `new`, the side the review read, is in the file at `path` now:
    /// in `now`, the open buffer's text, else in the work tree's file; with `or_side`, `new`
    /// built as a buffer's text when the work tree has no such file.
    Carry {
        path: Box<[u8]>,
        new: Arc<[u8]>,
        line: u32,
        now: Option<Rope>,
        or_side: bool,
    },
    /// `bytes`, a side, built as a buffer's text.
    Side { bytes: Arc<[u8]> },
    /// Whether the work tree's file at `path` may be opened at its first line, as one the review
    /// shows no text of: a regular file with one name, reached through no link. Nothing is read.
    Look { path: Box<[u8]> },
}

/// What a jump found, or why it could not.
pub(crate) struct Jumped {
    pub(crate) ticket: u64,
    pub(crate) landed: Result<Landed, String>,
}

impl fmt::Debug for Jumped {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let landed = match &self.landed {
            Ok(Landed::Line { line, carried }) => format!("line {line}, {carried:?}"),
            Ok(Landed::Side(_)) => "a side's text".to_owned(),
            Err(why) => format!("failed: {why}"),
        };
        f.debug_struct("Jumped")
            .field("ticket", &self.ticket)
            .field("landed", &landed)
            .finish()
    }
}

/// Where a jump lands.
pub(crate) enum Landed {
    /// At line `line`, from 1, of the file's text now.
    Line { line: u32, carried: Carried },
    /// A side's text, compared with no text: the buffer takes it whole.
    Side(Box<Reloaded>),
}

/// How a line was carried to the text now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Carried {
    /// The text is as the review read it, or the line moved with the edits around it.
    Kept,
    /// The line itself was changed since: this is the start of what replaced it.
    Changed,
    /// It was not carried, for this reason, and is the line as the review read it.
    Not(&'static str),
}

/// Answers `jump` with the work tree at `top`, as a failed jump if it panics, so that each jump
/// asked is answered by its ticket and a request that took it between its files goes on.
pub(super) fn answer_at(top: Option<&std::path::Path>, jump: Jump) -> Jumped {
    let ticket = jump.ticket;
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let top = top.and_then(|top| Directory::open(top).ok());
        answer(jump, top.as_ref())
    }))
    .unwrap_or_else(|_| {
        log::warn!("a review's jump panicked");
        Jumped {
            ticket,
            landed: Err("the review's thread failed to find the line".into()),
        }
    })
}

/// Answers `jump`, reading the work tree's file under `top` when no text is handed in.
pub(crate) fn answer(jump: Jump, top: Option<&Directory>) -> Jumped {
    let landed = match jump.ask {
        Ask::Carry {
            path,
            new,
            line,
            now,
            or_side,
        } => match carry_to_now(&path, &new, line, now, top) {
            Err(Unopened::Gone) if or_side => Ok(Landed::Side(Box::new(side_text(&new)))),
            carried => carried.map_err(String::from),
        },
        Ask::Side { bytes } => Ok(Landed::Side(Box::new(side_text(&bytes)))),
        Ask::Look { path } => match work_tree_found(&path, top) {
            Ok(_) => Ok(Landed::Line {
                line: 1,
                carried: Carried::Kept,
            }),
            Err(why) => Err(why.into()),
        },
    };
    Jumped {
        ticket: jump.ticket,
        landed,
    }
}

fn carry_to_now(
    path: &[u8],
    new: &[u8],
    line: u32,
    now: Option<Rope>,
    top: Option<&Directory>,
) -> Result<Landed, Unopened> {
    let now = match now {
        // A buffer past a side's limit is not copied: the line is the one the review read.
        Some(text) if text.len() as u64 > MAX_BLOB => {
            let why = "the buffer is too large to carry the line through its edits";
            return Ok(Landed::Line {
                line,
                carried: Carried::Not(why),
            });
        }
        Some(text) => {
            let mut bytes = Vec::with_capacity(text.len());
            for chunk in text.chunks() {
                bytes.extend_from_slice(chunk.as_bytes());
            }
            bytes
        }
        None => match work_tree_file(path, top)? {
            Some(bytes) => bytes,
            None => {
                let why = "the file is too large to carry the line through its edits";
                return Ok(Landed::Line {
                    line,
                    carried: Carried::Not(why),
                });
            }
        },
    };
    let (line, carried) = carry(new, &now, line, Instant::now() + DEADLINE);
    Ok(Landed::Line { line, carried })
}

/// Why the work tree's file was not opened.
enum Unopened {
    /// There is none, reached through no link.
    Gone,
    /// For this reason.
    Refused(String),
}

impl From<Unopened> for String {
    fn from(why: Unopened) -> Self {
        match why {
            Unopened::Gone => "The file is no longer in the work tree".into(),
            Unopened::Refused(why) => why,
        }
    }
}

/// The work tree's file at `path`, opened as a reading opens it, or why it may not be.
fn work_tree_found(path: &[u8], top: Option<&Directory>) -> Result<(std::fs::File, u64), Unopened> {
    let refused = |why: &str| Unopened::Refused(why.into());
    let top = top.ok_or_else(|| refused("The work tree cannot be opened"))?;
    match side::find(top, path).map_err(failed)? {
        Found::File(file, stamp) => Ok((file, stamp.len)),
        Found::Shared => Err(refused(
            "The file has more than one name, so the review does not open it",
        )),
        Found::Link(_) => Err(refused("A link: the review follows no link")),
        Found::Nothing => Err(Unopened::Gone),
    }
}

fn failed(e: std::io::Error) -> Unopened {
    Unopened::Refused(format!("The file cannot be read: {}", e.kind()))
}

/// The work tree's file at `path`, read as a reading reads it, `None` past a side's limit, or
/// why it may not be opened.
fn work_tree_file(path: &[u8], top: Option<&Directory>) -> Result<Option<Vec<u8>>, Unopened> {
    let (file, len) = work_tree_found(path, top)?;
    match side::read(file, len).map_err(failed)? {
        Got::Bytes(bytes) => Ok(Some(bytes)),
        Got::TooLarge(_) => Ok(None),
    }
}

/// Where line `line` of `new` is in `now`, and how it got there, within `deadline`.
pub(crate) fn carry(new: &[u8], now: &[u8], line: u32, deadline: Instant) -> (u32, Carried) {
    if new == now {
        return (line, Carried::Kept);
    }
    let Some(hunks) = diff::base_hunks(new, now, deadline) else {
        let why = if Instant::now() >= deadline {
            "not carried through the edits since the reading in time"
        } else {
            "the file is too long to carry the line through its edits"
        };
        return (line, Carried::Not(why));
    };
    let last = u32::try_from(diff::line_count(now).max(1)).unwrap_or(u32::MAX);
    let (line, carried) = shift(&hunks, line);
    (line.clamp(1, last), carried)
}

/// Carries `line` through `hunks`, from the side they change to the side they make: past every
/// hunk that ends before it, or to the start of the one that replaced it.
fn shift(hunks: &[BaseHunk], line: u32) -> (u32, Carried) {
    let spans = hunks.iter().map(|hunk| {
        let len = |range: &std::ops::Range<u32>| range.end.saturating_sub(range.start);
        (
            hunk.old.start,
            len(&hunk.old),
            hunk.new.start,
            len(&hunk.new),
        )
    });
    match through(spans, line) {
        (line, true) => (line, Carried::Changed),
        (line, false) => (line, Carried::Kept),
    }
}

/// Where line `number` of `from` is on the other side of a reading's `hunks`: where it was
/// removed or put in, for a line a hunk shows, else past the hunks before it.
pub(crate) fn across(hunks: &[Hunk], from: Side, number: u32) -> u32 {
    let len = |lines: &[Box<[u8]>]| u32::try_from(lines.len()).unwrap_or(u32::MAX);
    let spans = hunks.iter().map(|hunk| match from {
        Side::Old => (
            hunk.old_start,
            len(&hunk.removed),
            hunk.new_start,
            len(&hunk.added),
        ),
        Side::New => (
            hunk.new_start,
            len(&hunk.added),
            hunk.old_start,
            len(&hunk.removed),
        ),
    });
    through(spans, number).0
}

/// Carries line `number` through hunks in order, each its first line and length on this side
/// and on the other, with no context: to a hunk's first line on the other side for a line it
/// holds, else past those before it, one that holds no line of this side going in front of its
/// first. Says whether a hunk held the line. From 1.
fn through(hunks: impl Iterator<Item = (u32, u32, u32, u32)>, number: u32) -> (u32, bool) {
    let mut by: i64 = 0;
    for (start, len, there, there_len) in hunks {
        if start <= number && number < start.saturating_add(len) {
            return (there.max(1), true);
        }
        if start > number {
            break;
        }
        by += i64::from(there_len) - i64::from(len);
    }
    let line = u32::try_from(i64::from(number) + by).unwrap_or(1);
    (line.max(1), false)
}

/// `bytes` built as a buffer's text, as a file is loaded.
fn side_text(bytes: &[u8]) -> Reloaded {
    let mut builder = Builder::new(Some(bytes.len() as u64), TabWidth::default());
    builder.push(bytes);
    builder.finish();
    builder.snapshot().compared_with(&Rope::new())
}

#[cfg(test)]
#[path = "jump_tests.rs"]
mod tests;
