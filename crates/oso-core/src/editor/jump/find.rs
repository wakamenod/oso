//! Finding a jump's candidates in one window's rows in view (ADR 0116): a line's starts and
//! ends, or a string's matches, from the cursor's row outward, so that a budget that runs out
//! leaves out the farthest rows.
//!
//! Must not depend on: `command` or `keymap`, or the host's window or threads.

use std::ops::Range;
use std::time::Instant;

use super::Near;
use crate::buffer::Buffer;
use crate::display_map::DisplayPoint;
use crate::jump::{Candidate, Kind};
use crate::search::Needle;
use crate::sum_tree::Bias;
use crate::window::{Screen, WindowId};

/// Where [`candidates_in`] looks: the window's place among those looked at, the window, its
/// buffer, its rows in view, how many of them its text area shows, and its primary cursor's
/// point.
pub(super) struct Place<'a> {
    pub(super) rank: usize,
    pub(super) id: WindowId,
    pub(super) buffer: &'a Buffer,
    pub(super) screen: &'a Screen,
    pub(super) rows: usize,
    pub(super) cursor: DisplayPoint,
}

/// The rows of `count` from the cursor's row `at` outward: it, then one below and one above,
/// and so on, so that a budget that runs out leaves out the farthest rows.
pub(in crate::editor) fn outward(at: usize, count: usize) -> impl Iterator<Item = usize> {
    let first = (count > 0).then_some(at.min(count.saturating_sub(1)));
    let at = first.unwrap_or(0);
    let rest = (1..count).flat_map(move |step| {
        let below = Some(at + step).filter(|&row| row < count);
        [below, at.checked_sub(step)]
    });
    first.into_iter().chain(rest.flatten())
}

/// Adds the candidates of `kind` in a window's rows in view to `found`, reading at most
/// `budget` bytes, adding at most `most` and, for a regexp, looking until `until`, from the
/// cursor's row outward. Returns about how many bytes it read. `room` is room to work in: a
/// match's ranges and a line's text.
pub(super) fn candidates_in(
    place: &Place<'_>,
    (kind, needle): (Kind, Option<&Needle>),
    (budget, most, until): (usize, usize, Instant),
    found: &mut Vec<Near>,
    room: (&mut Vec<Range<usize>>, &mut String),
) -> usize {
    let display = place.buffer.display();
    let (screen, text) = (place.screen, place.buffer.text());
    let shown = &screen.rows()[..place.rows];
    let cursor_row = screen.row_of(place.cursor).unwrap_or(0);
    let limit = found.len().saturating_add(most);
    let push = |found: &mut Vec<Near>, range: Range<usize>, index: usize| {
        found.push(near(place, range, index.abs_diff(cursor_row)));
    };
    match (kind, needle) {
        (Kind::LineStart, _) => {
            for (index, row) in shown.iter().enumerate().take(most) {
                // A line's first row, with its first column in view.
                if row.starts_line() && screen.origin(row) == 0 {
                    push(found, row.start..row.start, index);
                }
            }
            0
        }
        (Kind::LineEnd, _) => {
            for (index, row) in shown.iter().enumerate().take(most) {
                // A line's last row, with its end, or the cell after its text, in view.
                if row.end_column != usize::MAX {
                    continue;
                }
                let end = text.line_end(row.line);
                let column = display.offset_to_display_point(end).column;
                if screen.drawn(row).contains(&column) {
                    push(found, end..end, index);
                }
            }
            0
        }
        (_, Some(needle)) => {
            let (ranges, kept) = room;
            let mut left = budget;
            // The rows of one line are matched together, as isearch's highlights are, so that
            // a match running into the next row hides the one it overlaps there.
            let mut done = 0..0;
            for index in outward(cursor_row, shown.len()) {
                if left == 0 || found.len() >= limit {
                    break;
                }
                if done.contains(&index) {
                    continue;
                }
                let line = shown[index].line;
                let first = (shown[..index].iter())
                    .rposition(|row| row.line != line)
                    .map_or(0, |at| at + 1);
                let last = index + shown[index..].iter().take_while(|r| r.line == line).count();
                done = if done.is_empty() {
                    first..last
                } else {
                    done.start.min(first)..done.end.max(last)
                };
                let within = (left, limit - found.len(), until);
                let run = Run {
                    needle,
                    rows: first..last,
                    cursor_row,
                };
                let read = run_matches(place, &run, within, found, (&mut *ranges, &mut *kept));
                left = left.saturating_sub(read);
            }
            budget - left
        }
        (_, None) => 0,
    }
}

/// A candidate found in `place` over `range`, `distance` rows from its cursor's.
fn near(place: &Place<'_>, range: Range<usize>, distance: usize) -> Near {
    Near {
        key: (place.rank, distance),
        column: 0,
        candidate: Candidate {
            window: place.id,
            range,
            start: DisplayPoint::default(),
            end: DisplayPoint::default(),
        },
    }
}

/// One line's rows in view, as [`run_matches`] matches them: what it looks for, the rows, and
/// the cursor's row.
struct Run<'a> {
    needle: &'a Needle,
    rows: Range<usize>,
    cursor_row: usize,
}

/// Adds to `found` the matches of `run`'s needle that start in its rows' columns in view, each
/// with its row, reading at most `budget` bytes, adding at most `most` and, for a regexp,
/// looking until `until`. The rows are matched as one span from the first's start, widened as
/// isearch's highlights are, so no two of them overlap. Returns about how many bytes it read.
/// `room` is room to work in.
fn run_matches(
    place: &Place<'_>,
    run: &Run<'_>,
    (budget, most, until): (usize, usize, Instant),
    found: &mut Vec<Near>,
    (ranges, line): (&mut Vec<Range<usize>>, &mut String),
) -> usize {
    let display = place.buffer.display();
    let (screen, text) = (place.screen, place.buffer.text());
    let rows = &screen.rows()[run.rows.clone()];
    let Some(first) = rows.first() else {
        return 0;
    };
    let offset =
        |column| display.display_point_to_offset(DisplayPoint::new(first.line, column), Bias::Left);
    // Where row `at` of the run starts and ends in the buffer, as its columns in view say.
    let bounds = |at: usize| {
        let drawn = screen.drawn(&rows[at]);
        offset(drawn.start)..offset(drawn.end)
    };
    let (start, end) = (bounds(0).start, bounds(rows.len() - 1).end);
    let reach = run.needle.reach();
    let span = start.saturating_sub(reach)..end.saturating_add(reach).min(text.len());
    ranges.clear();
    let invalid = place.buffer.invalid_bytes();
    let one = std::slice::from_ref(&span);
    run.needle
        .matches_in((text, invalid), one, (most, budget), (until, line), ranges);
    let (mut at, mut row) = (0, bounds(0));
    for range in ranges.drain(..) {
        while range.start >= row.end && at + 1 < rows.len() {
            at += 1;
            row = bounds(at);
        }
        // A match must start within its row's columns in view, though it may reach past them.
        if row.contains(&range.start) {
            let index = run.rows.start + at;
            found.push(near(place, range, index.abs_diff(run.cursor_row)));
        }
    }
    span.len().max(1)
}
