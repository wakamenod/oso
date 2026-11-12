//! Where a line's code ends, for `C-e` (ADR 0030, ADR 0111): before its trailing blanks, and
//! before a comment that ends the line after code, with the blanks between them. The comment
//! is what highlighting says, so a comment marker inside a string is no comment, and with no
//! highlights the code ends where the text does.
//!
//! Must not depend on: `buffer`, `window` or `editor`; it reads a display snapshot and the
//! highlights it is given.
//!
//! - One seek in the highlights, at the line's last byte before its blanks: neighbouring runs
//!   never have the same highlight, so that run is the whole comment.
//! - A line of nothing but a comment, or the inside of a block comment that began on an
//!   earlier line, has no code to stop at, and ends where its text does.
//! - Just after a comment marker is typed, the highlights are a guess until the next report,
//!   which may not have seen the comment yet.

use super::{WalkBudget, end_before_blanks, line_end, line_start};
use crate::display_map::DisplaySnapshot;
use crate::syntax::{Highlight, Highlights};

/// Where the code of `row` ends: before its trailing blanks, and before a comment that
/// `highlights` say ends the row after code, and before the blanks before that comment.
pub(super) fn code_end(
    display: &DisplaySnapshot,
    row: usize,
    highlights: Option<&Highlights>,
    budget: &mut WalkBudget,
) -> usize {
    let text_end = end_before_blanks(display, (row, line_end(display, row)), budget);
    let start = line_start(display, row);
    let Some(highlights) = highlights.filter(|_| text_end > start) else {
        return text_end;
    };
    let Some((comment, Some(Highlight::Comment))) = highlights.run_at(text_end - 1) else {
        return text_end;
    };
    if comment.start <= start {
        return text_end;
    }
    let code_end = end_before_blanks(display, (row, comment.start), budget);
    if code_end == start {
        return text_end;
    }
    code_end
}

#[cfg(test)]
mod tests;
