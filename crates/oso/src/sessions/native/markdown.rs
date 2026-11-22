//! A reply's Markdown highlights on its row (ADR 0165, part 2g; ADR 0111): which replies are
//! highlighted, and the runs a highlighted reply's text gives over its row as drawn.
//!
//! Must not depend on: the syntax threads, the editor or the theme; the host parses the text
//! and puts the runs in the buffer.
//!
//! - A reply is highlighted only once it is whole: its block closed, by its message or by the
//!   end of its turn, and its text at most [`MAX_REPLY`] bytes. A reply that streams stays
//!   plain.
//! - Its row draws each line of its text after a mark (`● `, or two spaces past the first
//!   line, after the row's indent), an empty line's without its spaces at the end, as `render`
//!   does; the projection walks the text's lines and
//!   the highlights' runs together, shifting each line's runs past its mark, which keeps its
//!   own face. A line's break, and the `\r` before it, have no run. O(runs + lines).
//! - Past [`MAX_RUNS`] runs the reply stays plain, which bounds what the UI thread puts in.

use std::ops::Range;

use oso_core::syntax::{Highlight, Highlights};

use super::render::{Face, blank_mark};
use super::rows::{Kind, Row};

/// The longest reply highlighted, in bytes (the author's choice): a longer one stays plain.
pub const MAX_REPLY: usize = 64 << 10;

/// The most runs a reply's highlights may put on its row: past it, the reply stays plain.
pub const MAX_RUNS: usize = 64 << 10;

/// The text of `row` to highlight: a reply whose block closed, of at most [`MAX_REPLY`] bytes,
/// not cut short. `None` for any other row.
pub fn highlightable(row: &Row) -> Option<&str> {
    match &row.kind {
        Kind::Reply {
            text,
            streaming: false,
            cut: false,
        } if text.len() <= MAX_REPLY => Some(text),
        _ => None,
    }
}

/// A run: a length in bytes and what those bytes are.
type Run = (usize, Option<Highlight>);

/// The runs over `row` as `render` draws it, from `highlights` over its text: each line's mark
/// in its own face, then the line's runs. `None` when `row` is not one [`highlightable`] gives,
/// when `highlights` do not cover its text, or past [`MAX_RUNS`].
pub fn project(row: &Row, highlights: &Highlights) -> Option<Vec<Run>> {
    let text = highlightable(row)?;
    if highlights.len() != text.len() {
        return None;
    }
    lay(row, text, |range, runs| {
        for (range, highlight) in highlights.runs(range) {
            if runs.len() >= MAX_RUNS {
                return false;
            }
            runs.push((range.len(), highlight));
        }
        true
    })
}

/// The runs of a reply's row with no Markdown, as `render` draws it: each line's mark in its
/// face and the rest plain, with no text rendered. What a reply's faces are laid again with
/// once highlighting is back on; `None` for a row [`highlightable`] does not give.
pub fn plain(row: &Row) -> Option<Vec<Run>> {
    lay(row, highlightable(row)?, |range, runs| {
        runs.push((range.len(), None));
        true
    })
}

/// Each line of `text`, reply `row`'s, as `render` draws it: its mark, then what `line` puts
/// for the line's bytes of `text`, then its break. `None` once `line` says no, or past
/// [`MAX_RUNS`].
fn lay(
    row: &Row,
    text: &str,
    mut line: impl FnMut(Range<usize>, &mut Vec<Run>) -> bool,
) -> Option<Vec<Run>> {
    let indent = "  ".repeat(usize::from(row.depth));
    // As `tail` draws them.
    let (first, rest) = (format!("{indent}● "), format!("{indent}  "));
    let face = Some(Face::Dim.highlight());
    let mut runs = Vec::new();
    let mark = |runs: &mut Vec<Run>, mark: &str, empty: bool| {
        let width = blank_mark(mark, empty).len();
        if width > 0 {
            runs.push((width, face));
        }
    };
    let mut at = 0;
    for (index, whole) in text.split_inclusive('\n').enumerate() {
        // As `str::lines`: a line's break, and a `\r` just before it, are not its text.
        let shown = whole
            .strip_suffix('\n')
            .map_or(whole, |shown| shown.strip_suffix('\r').unwrap_or(shown));
        let before = if index == 0 { &first } else { &rest };
        mark(&mut runs, before, shown.is_empty());
        if !line(at..at + shown.len(), &mut runs) {
            return None;
        }
        runs.push((1, None));
        at += whole.len();
        // Marks and breaks count too: many short lines are as many runs.
        if runs.len() > MAX_RUNS {
            return None;
        }
    }
    if text.is_empty() {
        mark(&mut runs, &first, true);
        runs.push((1, None));
    }
    Some(runs)
}

#[cfg(test)]
mod tests;
