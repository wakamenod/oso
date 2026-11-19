//! The unchanged lines a file's section shows around its hunks (ADR 0122, step 13): three
//! before and after each hunk, as GitHub shows them, and those the person asked to see with
//! the context keys; each stretch of the rest is one gap's row, but a stretch of one line,
//! which shows instead.
//!
//! Must not depend on: `app`, `dispatch`, git or the file system; this lays out a reading the
//! view thread holds.
//!
//! - The lines come from the reading's new side, which has every unchanged line the old one
//!   has; a gap's lines are numbered on both sides from the hunks around it.
//! - What the person asked to see is kept by the old side's numbers, which stay the same
//!   across readings against one old side: a refresh keeps it with no mapping, and a reading
//!   against another old side drops it.
//! - Context is shown only between and around the hunks of a text reading: a file that may
//!   hold secrets, a binary or too large one, a file with no hunks, and one whose new side has
//!   fewer lines than its hunks show none.
//! - Context rows are for showing only: staging builds its own patch, and comments, decisions
//!   and sending go by hunks and changed lines, never by these rows.

use std::ops::Range;

use oso_core::review::{Base, Hunk, RowKind, Side};
use oso_core::syntax::Highlight;

use super::State;
use super::section::{Writer, without_ending};
use crate::review::Content;

/// How many unchanged lines are shown before and after each hunk.
pub(crate) const CONTEXT: u32 = 3;

/// The most stretches of lines asked for a file that are kept; past it, the first asked go.
const MAX_REVEALED: usize = 256;

/// The unchanged lines the person asked to see of a file, by the old side's numbers, sorted
/// and apart, and the old side they were asked against.
#[derive(Debug, Default)]
pub(super) struct Revealed {
    ranges: Vec<Range<u32>>,
    base: Option<Base>,
}

impl Revealed {
    /// Shows `old`'s lines too, asked against the old side `base`: what was asked against
    /// another goes.
    pub(super) fn add(&mut self, old: Range<u32>, base: &Base) {
        if self.base.as_ref() != Some(base) {
            self.ranges.clear();
            self.base = Some(base.clone());
        }
        if old.is_empty() {
            return;
        }
        if self.ranges.len() >= MAX_REVEALED {
            self.ranges.remove(0);
        }
        // Every range that meets or touches it joins it.
        let from = self.ranges.partition_point(|range| range.end < old.start);
        let to = self.ranges.partition_point(|range| range.start <= old.end);
        let mut joined = old;
        if from < to {
            joined.start = joined.start.min(self.ranges[from].start);
            joined.end = joined.end.max(self.ranges[to - 1].end);
        }
        self.ranges.splice(from..to, [joined]);
    }

    /// Back to the hunks' own context.
    pub(super) fn clear(&mut self) {
        *self = Revealed::default();
    }

    /// What was asked against `base`, or nothing if it was asked against another.
    pub(super) fn against(&self, base: &Base) -> &[Range<u32>] {
        if self.base.as_ref() == Some(base) {
            &self.ranges
        } else {
            &[]
        }
    }
}

/// Where each line of a side starts, for its lines to be shown by number. A side is at most
/// [`MAX_BLOB`](crate::review::MAX_BLOB) bytes, so an offset fits a `u32`.
#[derive(Debug)]
pub(super) struct Lines {
    starts: Vec<u32>,
}

impl Lines {
    pub(super) fn of(text: &[u8]) -> Lines {
        let mut starts = Vec::with_capacity(text.len() / 32 + 1);
        if !text.is_empty() {
            starts.push(0);
        }
        for (at, &byte) in text.iter().enumerate() {
            if byte == b'\n' && at + 1 < text.len() {
                starts.push(u32::try_from(at + 1).unwrap_or(u32::MAX));
            }
        }
        Lines { starts }
    }

    /// Whether the side has every line `hunks` put in, as the side they were diffed to has:
    /// context is shown only from such a side.
    pub(super) fn holds(&self, hunks: &[Hunk]) -> bool {
        (hunks.last())
            .is_none_or(|hunk| end(hunk.new_start, &hunk.added) <= self.count().saturating_add(1))
    }

    /// How many lines the side has.
    fn count(&self) -> u32 {
        u32::try_from(self.starts.len()).unwrap_or(u32::MAX)
    }

    /// Line `number` of `text`, counted from 1, with its line break: empty past the last.
    fn line<'a>(&self, text: &'a [u8], number: u32) -> &'a [u8] {
        let Some(index) = (number as usize).checked_sub(1) else {
            return &[];
        };
        let Some(&start) = self.starts.get(index) else {
            return &[];
        };
        let end = self
            .starts
            .get(index + 1)
            .map_or(text.len(), |&end| end as usize);
        text.get(start as usize..end).unwrap_or_default()
    }
}

/// What a section shows of a reading's unchanged lines: its new side, where each of its lines
/// starts, and the old side's lines the person asked to see.
#[derive(Clone, Copy)]
pub(crate) struct Around<'a> {
    pub(super) new: &'a [u8],
    pub(super) lines: &'a Lines,
    pub(super) revealed: &'a [Range<u32>],
}

/// A stretch of unchanged lines, numbered from `old` and `new`: shown, or one gap's row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Piece {
    Shown { old: u32, new: u32, lines: u32 },
    Hidden { old: u32, new: u32, lines: u32 },
}

/// The unchanged lines before hunk `index` of `hunks`, or after the last one when `index` is
/// their count, as stretches shown and hidden in order. None for a reading with no hunk.
pub(super) fn pieces(hunks: &[Hunk], index: usize, around: &Around) -> Vec<Piece> {
    if hunks.is_empty() {
        return Vec::new();
    }
    let (old, new) = match index.checked_sub(1).and_then(|before| hunks.get(before)) {
        Some(hunk) => (
            end(hunk.old_start, &hunk.removed),
            end(hunk.new_start, &hunk.added),
        ),
        None => (1, 1),
    };
    let until = hunks
        .get(index)
        .map_or(around.lines.count().saturating_add(1), |hunk| {
            hunk.new_start
        });
    let lines = until.saturating_sub(new);
    if lines == 0 {
        return Vec::new();
    }
    // The offsets into the gap that are shown, as ranges in order.
    let mut shown: Vec<Range<u32>> = Vec::new();
    if index > 0 {
        shown.push(0..CONTEXT.min(lines));
    }
    let gap_end = old.saturating_add(lines);
    let from = around.revealed.partition_point(|range| range.end <= old);
    for range in &around.revealed[from..] {
        if range.start >= gap_end {
            break;
        }
        shown.push(range.start.max(old) - old..range.end.min(gap_end) - old);
    }
    if index < hunks.len() {
        shown.push(lines.saturating_sub(CONTEXT)..lines);
    }
    shown.sort_by_key(|range| range.start);
    let mut pieces = Vec::new();
    let mut at = 0;
    // A gap's row of one line would take its line's room: the line shows instead, joined to
    // the lines shown before it.
    let push = |pieces: &mut Vec<Piece>, from: u32, to: u32, hidden: bool| {
        if to <= from {
            return;
        }
        let (old, new, lines) = (old + from, new + from, to - from);
        if hidden && lines > 1 {
            pieces.push(Piece::Hidden { old, new, lines });
        } else if let Some(Piece::Shown { lines: before, .. }) = pieces.last_mut() {
            *before += lines;
        } else {
            pieces.push(Piece::Shown { old, new, lines });
        }
    };
    for range in shown {
        if range.end <= at {
            continue;
        }
        push(&mut pieces, at, range.start.max(at), true);
        push(&mut pieces, range.start.max(at), range.end, false);
        at = range.end;
    }
    push(&mut pieces, at, lines, true);
    pieces
}

/// The number just past a hunk's lines of one side that start at `start`.
fn end(start: u32, lines: &[Box<[u8]>]) -> u32 {
    start.saturating_add(u32::try_from(lines.len()).unwrap_or(u32::MAX))
}

/// Writes the unchanged lines before hunk `index`, or after the last, into `writer`: each
/// shown line plain with a space for its sign, its text taking `side`'s highlights, and a
/// gap's row for each stretch hidden.
pub(super) fn write(
    writer: &mut Writer,
    (hunks, index): (&[Hunk], usize),
    around: &Around,
    side: Side,
) {
    for piece in pieces(hunks, index, around) {
        match piece {
            Piece::Shown { old, new, lines } => {
                let kind = RowKind::Context { old, new };
                for at in 0..lines {
                    let (old, new) = (old + at, new + at);
                    let (text, _) = without_ending(around.lines.line(around.new, new));
                    let number = match side {
                        Side::Old => old,
                        Side::New => new,
                    };
                    writer.side_line(kind, (side, number), (b" ", text), None, &[]);
                }
            }
            Piece::Hidden { old, new, lines } => {
                let text = match lines {
                    1 => "\u{22ef} 1 unchanged line".to_owned(),
                    n => format!("\u{22ef} {n} unchanged lines"),
                };
                writer.line(
                    RowKind::Gap { old, new, lines },
                    &[(text.as_bytes(), Some(Highlight::DiffHunkHeader))],
                );
            }
        }
    }
}

impl State {
    /// Shows the old side's lines `show` of the file at `path` too, or only its hunks' own
    /// context again, laying out that file alone. What its reading does not show as text is
    /// left as it is.
    pub(super) fn reveal(&mut self, path: &[u8], show: Option<Range<u32>>) {
        let Some(file) = self.files.get_mut(path) else {
            return;
        };
        let Some(Content::Text { shape, .. }) = file.reading.as_deref().map(|it| &it.content)
        else {
            return;
        };
        match show {
            Some(old) => file.revealed.add(old, shape.base()),
            None => file.revealed.clear(),
        }
        file.rendered.clear();
        self.dirty = true;
    }
}

#[cfg(test)]
#[path = "context_tests.rs"]
mod tests;
