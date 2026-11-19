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
//!   hold secrets, by its name or as an open buffer of it may, a binary or too large one, a file
//!   with no hunks, and one whose new side has fewer lines than its hunks show none.
//! - The lines shown are told to the sides' highlighting, which projects their runs too; the old
//!   side's only while the panes are built.
//! - A key's lines come from the rows on screen; asked for again before the text the last press
//!   asked for went in, lines all shown already, they are taken as the next ones that way.
//! - Context rows are for showing only: staging builds its own patch, and comments, decisions
//!   and sending go by hunks and changed lines, never by these rows.

use std::collections::HashSet;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use oso_core::review::{Base, Hunk, RowKind, Side};
use oso_core::syntax::Highlight;

use super::State;
use super::section::{Writer, without_ending};
use crate::review::Content;

/// How many unchanged lines are shown before and after each hunk.
const CONTEXT: u32 = 3;

/// The most stretches of lines asked for a file that are kept; past it, the topmost other
/// than the one just asked goes.
const MAX_REVEALED: usize = 256;

/// How many lines apart [`Lines`] keeps where a line starts.
const EVERY: usize = 32;

/// The unchanged lines the person asked to see of a file, by the old side's numbers, sorted
/// and apart, and the old side they were asked against.
#[derive(Debug, Default)]
pub(super) struct Revealed {
    ranges: Vec<Range<u32>>,
    base: Option<Base>,
}

impl Revealed {
    /// Shows `old`'s lines too, asked against the old side `base`: what was asked against
    /// another goes. Whether what shows changed.
    pub(super) fn add(&mut self, old: Range<u32>, base: &Base) -> bool {
        let mut changed = false;
        if self.base.as_ref() != Some(base) {
            changed = !self.ranges.is_empty();
            self.ranges.clear();
            self.base = Some(base.clone());
        }
        if old.is_empty() {
            return changed;
        }
        // Every range that meets or touches it joins it.
        let from = self.ranges.partition_point(|range| range.end < old.start);
        let to = self.ranges.partition_point(|range| range.start <= old.end);
        let mut joined = old;
        if from < to {
            joined.start = joined.start.min(self.ranges[from].start);
            joined.end = joined.end.max(self.ranges[to - 1].end);
            if to - from == 1 && self.ranges[from] == joined {
                return changed;
            }
        }
        self.ranges.splice(from..to, [joined]);
        if self.ranges.len() > MAX_REVEALED {
            self.ranges.remove(usize::from(from == 0));
        }
        true
    }

    /// Back to the hunks' own context; whether anything was asked.
    pub(super) fn clear(&mut self) -> bool {
        let changed = !self.ranges.is_empty();
        *self = Revealed::default();
        changed
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

/// Where every [`EVERY`]th line of a side starts, and how many lines it has, for its lines to
/// be shown by number: a line is found from the nearest start kept before it. A side is at most
/// [`MAX_BLOB`](crate::review::MAX_BLOB) bytes, so an offset fits a `u32`, and this holds at
/// most 4 bytes for each 32 lines.
#[derive(Debug)]
pub(super) struct Lines {
    starts: Vec<u32>,
    count: u32,
}

impl Lines {
    pub(super) fn of(text: &[u8]) -> Lines {
        let count = crate::review::diff::line_count(text);
        let mut starts = Vec::with_capacity(count.div_ceil(EVERY));
        let breaks = (text.iter().enumerate()).filter(|(_, byte)| **byte == b'\n');
        let mut nth = 0;
        if count > 0 {
            starts.push(0);
        }
        for (at, _) in breaks {
            nth += 1;
            if nth % EVERY == 0 && nth < count {
                starts.push(u32::try_from(at + 1).unwrap_or(u32::MAX));
            }
        }
        let count = u32::try_from(count).unwrap_or(u32::MAX);
        Lines { starts, count }
    }

    /// Whether the side has every line `hunks` put in, as the side they were diffed to has:
    /// context is shown only from such a side.
    pub(super) fn holds(&self, hunks: &[Hunk]) -> bool {
        (hunks.last())
            .is_none_or(|hunk| end(hunk.new_start, &hunk.added) <= self.count.saturating_add(1))
    }

    /// The lines of `text` from number `first`, counted from 1, each with its line break, as
    /// many as it has from there.
    fn from<'a>(&self, text: &'a [u8], first: u32) -> impl Iterator<Item = &'a [u8]> {
        let index = (first as usize).saturating_sub(1);
        let kept = self
            .starts
            .get(index / EVERY)
            .map_or(text.len(), |&at| at as usize);
        let rest = text.get(kept..).unwrap_or_default();
        let skip = if first == 0 {
            usize::MAX
        } else {
            index % EVERY
        };
        rest.split_inclusive(|&byte| byte == b'\n').skip(skip)
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
        .map_or(around.lines.count.saturating_add(1), |hunk| hunk.new_start);
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
                let first = match side {
                    Side::Old => old,
                    Side::New => new,
                };
                let texts = around.lines.from(around.new, new).take(lines as usize);
                for (number, line) in (first..).zip(texts) {
                    let (text, _) = without_ending(line);
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

/// The unchanged lines `pieces` shows around every hunk of `hunks`, as ranges of each side's
/// numbers, old then new; none of the old side unless `old`, as only the old pane shows them.
fn shown_lines(hunks: &[Hunk], around: &Around, old: bool) -> [Vec<Range<u32>>; 2] {
    let mut lines = [Vec::new(), Vec::new()];
    for index in 0..=hunks.len() {
        for piece in pieces(hunks, index, around) {
            if let Piece::Shown {
                old: from_old,
                new,
                lines: count,
            } = piece
            {
                if old {
                    lines[0].push(from_old..from_old + count);
                }
                lines[1].push(new..new + count);
            }
        }
    }
    lines
}

/// The old side's lines to show for a key that asked for `asked`: those, unless every one of
/// them shows already, as a key pressed again before the text the last press asked for went in
/// reads the rows that text replaces. Then as many lines as it asked for, next to the stretch
/// shown that holds them, on its side where lines are still hidden: after it when only that
/// side is, or both are and `asked` ends it; before it when only that side is, or both are and
/// `asked` starts it; else none.
fn asked_now(hunks: &[Hunk], around: &Around, asked: Range<u32>) -> Range<u32> {
    if asked.is_empty() {
        return asked;
    }
    // The gap that holds its first line: the one before the first hunk that starts after it.
    let index = hunks.partition_point(|hunk| hunk.old_start <= asked.start);
    let pieces = pieces(hunks, index, around);
    let shown_at = pieces.iter().position(|piece| {
        matches!(*piece, Piece::Shown { old, lines, .. }
            if old <= asked.start && asked.end <= old.saturating_add(lines))
    });
    let Some(at) = shown_at else {
        // Some of it is hidden, or it is no gap's: as asked.
        return asked;
    };
    let Piece::Shown { old, lines, .. } = pieces[at] else {
        return asked;
    };
    let end = old.saturating_add(lines);
    let hidden = |piece: Option<&Piece>| matches!(piece, Some(Piece::Hidden { .. }));
    let (before, after) = (
        hidden(at.checked_sub(1).and_then(|it| pieces.get(it))),
        hidden(pieces.get(at + 1)),
    );
    let count = asked.end - asked.start;
    let forward = match (before, after) {
        (_, true) if !before || asked.end == end || asked.start != old => true,
        (true, _) => false,
        _ => return asked.start..asked.start,
    };
    if forward {
        end..end.saturating_add(count)
    } else {
        old.saturating_sub(count).max(1)..old
    }
}

/// Whether the file at `path`, as git gives it under `top`, is one of `held`.
fn is_held(held: &HashSet<PathBuf>, top: &Path, path: &[u8]) -> bool {
    !held.is_empty() && crate::git::os_path(path).is_ok_and(|path| held.contains(&top.join(path)))
}

impl State {
    /// After the file at `path` shows a reading, or what it shows around its hunks changed:
    /// whether an open buffer of it may hold secrets, and the unchanged lines its sides'
    /// highlighting covers.
    pub(super) fn context_shown(&mut self, path: &[u8]) {
        let held = is_held(&self.held_buffers, self.highlighting.top(), path);
        let old = self.panes_on();
        let Some(file) = self.files.get_mut(path) else {
            return;
        };
        if file.held != held {
            file.held = held;
            file.rendered.clear();
            self.dirty = true;
        }
        let lines = (file.around()).map_or_else(Default::default, |(hunks, around)| {
            shown_lines(hunks, &around, old)
        });
        self.highlighting.context(path, lines);
    }

    /// Takes the files of the open buffers that may hold secrets: those that change show their
    /// unchanged lines, or no more.
    pub(super) fn held_buffers(&mut self, held: Arc<HashSet<PathBuf>>) {
        self.held_buffers = held;
        for path in self.order.clone() {
            self.context_shown(&path);
        }
    }

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
        let changed = match show {
            Some(old) => {
                let old = match file.around() {
                    Some((hunks, around)) => asked_now(hunks, &around, old),
                    None => old,
                };
                file.revealed.add(old, shape.base())
            }
            None => file.revealed.clear(),
        };
        if changed {
            file.rendered.clear();
            self.dirty = true;
            self.context_shown(path);
        }
    }
}

#[cfg(test)]
#[path = "context_repeat_tests.rs"]
mod repeat_tests;
#[cfg(test)]
#[path = "context_tests.rs"]
mod tests;
