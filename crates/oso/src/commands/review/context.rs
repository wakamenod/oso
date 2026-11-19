//! The context keys of a review buffer (ADR 0122, step 13): `[` shows more of the unchanged
//! lines hidden above the point, `]` more of those below, `=` all of a gap, and
//! `review-reset-context` only the three lines around each hunk again.
//!
//! Must not depend on: the host's threads; this finds the gaps by the rows, never the text,
//! and asks the host, whose view lays the file out again.
//!
//! - Off a gap's row, `[` takes the nearest gap above the point and shows its last lines, those
//!   next to what is shown; `]` the nearest below and its first lines. On a gap's row, `[` shows
//!   its first lines, above the row, and `]` its last, below it.
//! - Each shows [`STEP`] lines, four times more for each `C-u` alone, or as many as a number
//!   given says, at most [`MOST`]; a gap of no more shows whole. A negative number shows as many,
//!   the same way: the key says which way.
//! - They work on every cursor, and are `Change`: they show only lines of a file the review
//!   read, as the person could scroll to; a file that may hold secrets has no gaps.

use std::ops::Range;

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Prefix, Reach};
use oso_core::editor::{ReviewId, ReviewRequest};
use oso_core::review::{RowKind, Rows, Seek};

use super::current_review;

/// How many hidden lines a context key shows with no prefix argument.
const STEP: u32 = 20;
/// The most hidden lines one press shows.
const MOST: u32 = 10_000;

/// The commands that show unchanged lines around the hunks.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "review-context-up",
        doc: "Show 20 more unchanged lines above every cursor: the last lines of the nearest gap above it, or on a gap's row its first lines.\n\nWith a numeric argument N, show N lines; each `C-u` alone shows four times more. At most 10000 lines show at once. A gap of no more shows whole.",
        args: ArgSpec::Count,
        reach: Reach::Change,
        run: review_context_up,
        state: None,
    },
    Command {
        name: "review-context-down",
        doc: "Show 20 more unchanged lines below every cursor: the first lines of the nearest gap below it, or on a gap's row its last lines.\n\nWith a numeric argument N, show N lines; each `C-u` alone shows four times more. At most 10000 lines show at once. A gap of no more shows whole.",
        args: ArgSpec::Count,
        reach: Reach::Change,
        run: review_context_down,
        state: None,
    },
    Command {
        name: "review-context-gap",
        doc: "Show every unchanged line of the gap at each cursor, or of the nearest gap below it.\n\nA gap of more than 10000 lines shows its first 10000.",
        args: ArgSpec::None,
        reach: Reach::Change,
        run: review_context_gap,
        state: None,
    },
    Command {
        name: "review-reset-context",
        doc: "Show only the three unchanged lines around each hunk again, in the file at every cursor.",
        args: ArgSpec::None,
        reach: Reach::Change,
        run: review_reset_context,
        state: None,
    },
];

/// Which hidden lines a key shows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Toward {
    Up,
    Down,
    Whole,
}

/// The old side's lines the key `toward` shows at line `line` of `rows`, `count` of them, and
/// the file they are in: `None` where no gap of that file is that way.
fn shown_by(
    rows: &Rows,
    line: usize,
    toward: Toward,
    count: u32,
) -> Option<(Box<[u8]>, Range<u32>)> {
    let row = rows.at(line)?;
    if row.path.is_empty() {
        return None;
    }
    // Which gap, and whether its first lines show (`Some(true)`), its last, or all.
    let (at, first) = match (row.kind, toward) {
        (RowKind::Gap { .. }, Toward::Up) => (line, Some(true)),
        (RowKind::Gap { .. }, Toward::Down) => (line, Some(false)),
        (RowKind::Gap { .. }, Toward::Whole) => (line, None),
        (_, Toward::Up) => (rows.seek(line, Seek::Gap, false)?, Some(false)),
        (_, Toward::Down) => (rows.seek(line, Seek::Gap, true)?, Some(true)),
        (_, Toward::Whole) => (rows.seek(line, Seek::Gap, true)?, None),
    };
    let gap = rows.at(at)?;
    let RowKind::Gap { old, lines, .. } = gap.kind else {
        return None;
    };
    if gap.path != row.path {
        return None;
    }
    let count = match first {
        Some(_) => count.clamp(1, MOST).min(lines),
        None => lines.min(MOST),
    };
    let end = old.saturating_add(lines);
    let range = match first {
        Some(false) => end - count..end,
        Some(true) | None => old..old.saturating_add(count),
    };
    Some((gap.path.into(), range))
}

/// How many lines a context key shows: [`STEP`], times 4 for each `C-u` alone, or a number
/// given, its sign dropped; at most [`MOST`].
fn count_of(args: Args) -> u32 {
    let count = match args.raw() {
        None => STEP,
        Some(Prefix::Universal(times)) => {
            STEP.saturating_mul(u32::try_from(times.unsigned_abs()).unwrap_or(u32::MAX))
        }
        Some(prefix) => u32::try_from(prefix.value().unsigned_abs()).unwrap_or(u32::MAX),
    };
    count.min(MOST)
}

/// Asks the host to show, for every cursor, the lines `toward` shows there.
fn show(ctx: &mut Ctx<'_>, toward: Toward, count: u32) -> Result<(), CommandError> {
    let review = current_review(ctx)?;
    let (rows, lines) = rows_and_lines(ctx)?;
    let mut asked: Vec<(Box<[u8]>, Range<u32>)> = (lines.into_iter())
        .filter_map(|line| shown_by(&rows, line, toward, count))
        .collect();
    // In the text's order, each once.
    asked.sort_by(|a, b| (&a.0, a.1.start, a.1.end).cmp(&(&b.0, b.1.start, b.1.end)));
    asked.dedup();
    if asked.is_empty() {
        return Err(CommandError::new(match toward {
            Toward::Up => "No hidden lines above",
            Toward::Down => "No hidden lines below",
            Toward::Whole => "No hidden lines here",
        }));
    }
    for (path, old) in asked {
        ask(ctx, review, path, Some(old));
    }
    Ok(())
}

/// The selected window's rows, and the line of each of its cursors.
fn rows_and_lines(ctx: &Ctx<'_>) -> Result<(Rows, Vec<usize>), CommandError> {
    let (window, buffer) = ctx.editor.current();
    // A clone of the rows is O(1): they are shared. A pane has rows of its own.
    let rows = (ctx.editor.review_rows_of(window.buffer()).cloned())
        .ok_or_else(|| CommandError::new("Not in a review"))?;
    let text = buffer.text();
    let lines = (window.selections.iter())
        .map(|selection| text.offset_to_point(selection.cursor).row)
        .collect();
    Ok((rows, lines))
}

fn ask(ctx: &mut Ctx<'_>, review: ReviewId, path: Box<[u8]>, show: Option<Range<u32>>) {
    ctx.editor
        .request_review(ReviewRequest::Context { review, path, show });
    ctx.request_redraw();
}

fn review_context_up(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    show(ctx, Toward::Up, count_of(args))
}

fn review_context_down(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    show(ctx, Toward::Down, count_of(args))
}

fn review_context_gap(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    show(ctx, Toward::Whole, MOST)
}

fn review_reset_context(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let review = current_review(ctx)?;
    let (rows, lines) = rows_and_lines(ctx)?;
    let mut paths: Vec<Box<[u8]>> = (lines.into_iter())
        .filter_map(|line| rows.at(line).filter(|row| !row.path.is_empty()))
        .map(|row| row.path.into())
        .collect();
    paths.sort();
    paths.dedup();
    if paths.is_empty() {
        return Err(CommandError::new("Not on a file"));
    }
    for path in paths {
        ask(ctx, review, path, None);
    }
    Ok(())
}

#[cfg(test)]
#[path = "context_tests.rs"]
mod tests;
