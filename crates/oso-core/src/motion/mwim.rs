//! `C-a` and `C-e` as mwim moves them (ADR 0030): to a line's first non-blank character or
//! its start, and to the end of its code or its end. The code ends before trailing blanks, and
//! before a comment that ends the line after code, with the blanks between them (ADR 0111). The
//! comment is what highlighting says, so a comment marker inside a string is no comment, and
//! with no highlights the code ends where the text does.
//!
//! Must not depend on: `buffer`, `window` or `editor`; it reads a display snapshot and the
//! highlights it is given.
//!
//! - One seek in the highlights, at the line's last byte before its blanks: neighbouring runs
//!   never have the same highlight, and no query captures anything inside a comment, so that
//!   run is the whole comment.
//! - A line of nothing but a comment, or the inside of a block comment that began on an
//!   earlier line, has no code to stop at, and ends where its text does.
//! - The code ends where a display cluster starts, though a comment may start inside one.
//! - The walks back over blanks, behind the comment and before it, are one stop's reads, and
//!   none is read once the stop or the command has nothing left (ADR 0058).
//! - Just after a comment marker is typed, the highlights are a guess until the next report,
//!   which may not have seen the comment yet.

use super::{
    SEEK_COST, StopWalk, WalkBudget, cluster_start, clusters_before, line_end, line_start,
    target_row, to_line_end,
};
use crate::display_map::{CellKind, DisplayCluster, DisplayPoint, DisplaySnapshot};
use crate::selection::Selection;
use crate::syntax::{Highlight, Highlights};

/// Moves `selection` as `C-a` does (mwim, ADR 0030): to the first non-blank
/// character of its line, or from there to the line's start. With `n` other than 1
/// it moves `n - 1` lines down first, and goes to the first non-blank character.
pub fn toggle_line_start(
    display: &DisplaySnapshot,
    selection: &mut Selection,
    n: isize,
    budget: &mut WalkBudget,
) {
    let row = target_row(display, selection.cursor, n);
    let code = first_non_blank(display, row, budget);
    selection.move_to(if n == 1 && selection.cursor == code {
        line_start(display, row)
    } else {
        code
    });
}

/// Moves `selection` as `C-e` does (mwim, ADR 0030): to the end of its line's code, before
/// trailing blanks and before a comment that `highlights` say ends the line (ADR 0111), or
/// from there to the line's end. With `n` other than 1 it moves `n - 1` lines down first, and
/// goes to the end of the code.
pub fn toggle_line_end(
    display: &DisplaySnapshot,
    selection: &mut Selection,
    n: isize,
    highlights: Option<&Highlights>,
    budget: &mut WalkBudget,
) {
    let row = target_row(display, selection.cursor, n);
    let end = line_end(display, row);
    let code = code_end(display, (row, end), highlights, &mut budget.stop());
    if code == end || (n == 1 && selection.cursor == code) {
        to_line_end(selection, end);
    } else {
        selection.move_to(code);
    }
}

/// Where the code of `row`, which ends at `end`, ends: before its trailing blanks, and before a
/// comment that `highlights` say ends the row after code, and before the blanks before that
/// comment.
fn code_end(
    display: &DisplaySnapshot,
    (row, end): (usize, usize),
    highlights: Option<&Highlights>,
    walk: &mut StopWalk<'_>,
) -> usize {
    let text_end = end_before_blanks(display, end, walk);
    let Some(highlights) = highlights.filter(|_| walk.has_left()) else {
        return text_end;
    };
    let start = line_start(display, row);
    if text_end <= start {
        return text_end;
    }
    let Some((comment, Some(Highlight::Comment))) = highlights.run_at(text_end - 1) else {
        return text_end;
    };
    // A run may start inside a display cluster, as text after a prepended character joins it:
    // the code ends at a cluster's start, where a cursor can be.
    let comment = cluster_start(display, comment.start);
    if comment <= start {
        return text_end;
    }
    let code_end = end_before_blanks(display, comment, walk);
    if code_end == start {
        return text_end;
    }
    code_end
}

/// Whether `cluster` is blank: a tab, or text of nothing but white space, such as a
/// full-width space. Such text with a mark on it is not.
fn is_blank(cluster: &DisplayCluster<'_>) -> bool {
    match cluster.kind {
        CellKind::Tab => true,
        CellKind::Text => cluster.text.chars().all(char::is_whitespace),
        _ => false,
    }
}

/// Where the first non-blank cluster of `row` starts, or the row's end if it has
/// none. A run of blanks past what a stop reads ends the search where it got to.
fn first_non_blank(display: &DisplaySnapshot, row: usize, budget: &mut WalkBudget) -> usize {
    let mut walk = budget.stop();
    let mut end = line_start(display, row);
    if !walk.spend(SEEK_COST) {
        return end;
    }
    for cluster in display.clusters(DisplayPoint::new(row, 0)) {
        if !is_blank(&cluster) || !walk.spend(cluster.text.len()) {
            return cluster.offset;
        }
        end = cluster.offset + cluster.text.len();
    }
    end
}

/// Where the text of the row up to `end` ends before the blanks it ends with, or the row's
/// start if it has nothing else. A run of blanks past what `walk` may read ends the search
/// where it got to.
fn end_before_blanks(display: &DisplaySnapshot, end: usize, walk: &mut StopWalk<'_>) -> usize {
    let mut point = display.offset_to_display_point(end);
    let row = point.row;
    let mut earliest = end;
    while point.column > 0 && walk.has_left() {
        let mut text_end = None;
        let mut first = None;
        let mut read = 0;
        for cluster in clusters_before(display, point, earliest) {
            first.get_or_insert((cluster.offset, cluster.column));
            read += cluster.text.len();
            if !is_blank(&cluster) {
                text_end = Some(cluster.offset + cluster.text.len());
            }
        }
        let spent = walk.spend(read + SEEK_COST);
        if let Some(text_end) = text_end {
            return text_end;
        }
        let Some((offset, column)) = first else {
            break;
        };
        earliest = offset;
        if !spent {
            return earliest;
        }
        point = DisplayPoint::new(row, column);
    }
    earliest
}

#[cfg(test)]
mod tests;
