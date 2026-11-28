//! Motion by paragraphs, as Emacs's `forward-paragraph` and `backward-paragraph` move with
//! their default `paragraph-start` and `paragraph-separate` (ADR 0190).
//!
//! Must not depend on: `buffer`, `window` or `editor`; it reads a display snapshot only.
//!
//! - A line of only spaces, tabs and form feeds separates paragraphs. A line that starts with
//!   a form feed starts one. In Markdown a heading or a fence line is a paragraph of its own.
//! - Forward, a paragraph ends at the start of the line after its last; back, it starts at the
//!   start of the separating line just above it, or of its first line where none is.
//! - Lines are read in order, at the cost of their bytes. A step that runs out of budget
//!   leaves the cursor where the step before it left it.

use std::ops::Range;

use super::text::{self, Line, lines_after, lines_before};
use super::{Edge, WalkBudget, line_end, line_start};
use crate::display_map::DisplaySnapshot;
use crate::selection::Selection;

/// Moves `selection` over `n` paragraphs, back if `n` is negative. A step at the buffer's
/// edge fails with it, after the steps before it have moved the cursor.
pub fn forward_paragraph(
    display: &DisplaySnapshot,
    markdown: bool,
    selection: &mut Selection,
    n: isize,
    budget: &mut WalkBudget,
) -> Result<(), Edge> {
    text::steps(display, selection, n, |at, forward| {
        if forward {
            paragraph_end(display, at, markdown, budget)
        } else {
            paragraph_start(display, at, markdown, budget)
        }
    })
}

/// The paragraph `mark-paragraph` marks at `offset`: from where `backward-paragraph` goes
/// from the end `forward-paragraph` finds, to that end. A step past the budget does not move.
pub fn paragraph_bounds(
    display: &DisplaySnapshot,
    offset: usize,
    markdown: bool,
    budget: &mut WalkBudget,
) -> Range<usize> {
    let end = if offset < display.len() {
        paragraph_end(display, offset, markdown, budget).unwrap_or(offset)
    } else {
        offset
    };
    let start = if end > 0 {
        paragraph_start(display, end, markdown, budget).unwrap_or(end)
    } else {
        0
    };
    start..end
}

/// Where the paragraph at or after `at`, before the buffer's end, ends: past the separating
/// lines `at`'s line starts with, and one line, at the next line that separates or starts a
/// paragraph, or after a line that stands apart; else the buffer's end. `None` if the budget
/// ran out first.
pub(super) fn paragraph_end(
    display: &DisplaySnapshot,
    at: usize,
    markdown: bool,
    budget: &mut WalkBudget,
) -> Option<usize> {
    let row_start = line_start(display, display.offset_to_display_point(at).row);
    if !text::enter(budget) {
        return None;
    }
    let mut lines = lines_after(display, row_start, markdown, budget);
    let mut before = loop {
        match lines.next_line() {
            Some((_, Line::Blank)) => {}
            Some((_, kind)) => break kind,
            None => return lines.at_end(),
        }
    };
    loop {
        match lines.next_line() {
            Some((start, kind)) if kind != Line::Text || before == Line::Alone => {
                return Some(start);
            }
            Some((_, kind)) => before = kind,
            None => return lines.at_end(),
        }
    }
}

/// Where the paragraph before `at`, after the buffer's start, starts: at the separating line
/// just above its first line, or at its first line where none is. From a paragraph's first
/// line, the separating line above it. `None` if the budget ran out first.
pub(super) fn paragraph_start(
    display: &DisplaySnapshot,
    at: usize,
    markdown: bool,
    budget: &mut WalkBudget,
) -> Option<usize> {
    let point = display.offset_to_display_point(at);
    if !text::enter(budget) {
        return None;
    }
    if point.column == 0 && point.row > 0 {
        let here = lines_after(display, at, markdown, budget).next_line()?;
        if here.1 != Line::Blank {
            if !text::seek(budget) {
                return None;
            }
            let above = lines_before(display, line_end(display, point.row - 1), markdown, budget);
            if let (start, Line::Blank) = above.into_first()? {
                return Some(start);
            }
        }
    }
    let row = if point.column == 0 {
        point.row - 1
    } else {
        point.row
    };
    if !text::seek(budget) {
        return None;
    }
    let mut lines = lines_before(display, line_end(display, row), markdown, budget);
    let (first, kind) = loop {
        match lines.next_line() {
            Some((start, Line::Blank)) if start > 0 => {}
            Some(line) => break line,
            None => return lines.at_start(),
        }
    };
    if first == 0 {
        return Some(0);
    }
    // A line that starts a paragraph, or stands apart, waits for the one above it: the
    // paragraph starts at that one if it separates.
    let mut waiting = matches!(kind, Line::Start | Line::Alone).then_some(first);
    let mut below = first;
    loop {
        let Some((start, kind)) = lines.next_line() else {
            return lines.at_start().map(|start| waiting.unwrap_or(start));
        };
        if let Some(own) = waiting {
            return Some(if kind == Line::Blank { start } else { own });
        }
        match kind {
            Line::Blank => return Some(start),
            Line::Alone => return Some(below),
            Line::Start => waiting = Some(start),
            Line::Text => {}
        }
        below = start;
    }
}

#[cfg(test)]
mod tests;
