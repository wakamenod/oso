//! A reply's Markdown highlights on its row (ADR 0165, part 2g; ADR 0111): which replies are
//! highlighted, and the runs a highlighted reply's text gives over its row as drawn.
//!
//! Must not depend on: the syntax threads, the editor or the theme; the host parses the text
//! and puts the runs in the buffer.
//!
//! - A reply is highlighted only once it is whole: its block closed, by its message or by the
//!   end of its turn, and its text at most [`MAX_REPLY`] bytes. A reply that streams has
//!   the faces it is drawn with.
//! - Its row draws each line as `shown` says, after its indent, an empty line's without its
//!   spaces at the end, and its gap's blank line before them, as `render` does; the projection
//!   walks the same lines. What Markdown's hidden marks are around keeps
//!   the face `shown` gives it. A fence's code takes its language's highlights, or code's face
//!   when its language gave it none; what is drawn as written takes the parse's, a `_` emphasis
//!   or a table's code among it. A line's break has no run. O(runs + lines).
//! - Past [`MAX_RUNS`] runs the reply stays as drawn, which bounds what the UI thread puts in.

use oso_core::syntax::{Highlight, Highlights};

use super::render::{Face, reply_lines};
use super::rows::{Kind, Row};
use super::shown::{LineStart, Piece, Style};

/// The longest reply highlighted, in bytes (the author's choice): a longer one stays plain.
pub const MAX_REPLY: usize = 64 << 10;

/// The most runs a reply's highlights may put on its row: past it, the reply
/// stays as drawn.
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

/// The runs over `row` as `render` draws it, with `highlights` over its text for its code and
/// for what is drawn as written: each line's mark in its own face, then the line's runs. `None`
/// when `row` is not one [`highlightable`] gives, when `highlights` do not cover its text, or
/// past [`MAX_RUNS`].
pub fn project(row: &Row, highlights: &Highlights) -> Option<Vec<Run>> {
    let text = highlightable(row)?;
    if highlights.len() != text.len() {
        return None;
    }
    let runs = lay(row, text, false, Some(highlights), MAX_RUNS);
    (runs.len() <= MAX_RUNS).then_some(runs)
}

/// The runs of a reply's row with no language's highlights, as `render` draws it, with no text
/// rendered: what a reply's faces are laid again with once highlighting is back on; `None` for
/// a row that is no reply, or one cut as it came.
pub fn plain(row: &Row) -> Option<Vec<Run>> {
    match &row.kind {
        Kind::Reply {
            text,
            streaming,
            cut: false,
        } => Some(lay(row, text, *streaming, None, usize::MAX)),
        _ => None,
    }
}

/// Each line of `text`, reply `row`'s, as `render` draws it: its mark, its pieces, then its
/// break; about `most` runs, then no more.
fn lay(
    row: &Row,
    text: &str,
    streams: bool,
    highlights: Option<&Highlights>,
    most: usize,
) -> Vec<Run> {
    let mark_run = Some(Face::Dim.highlight());
    let mut runs: Vec<Run> = Vec::new();
    if row.gap {
        runs.push((1, None));
    }
    let mut code = None;
    reply_lines(row, text, LineStart::default(), streams, |mark, pieces| {
        // Marks and breaks count too: many short lines are as many runs.
        if runs.len() > most {
            return;
        }
        if !mark.is_empty() {
            runs.push((mark.len(), mark_run));
        }
        for piece in pieces {
            put(&mut runs, &mut code, piece, highlights, most);
        }
        runs.push((1, None));
    });
    if let Some(code) = code {
        code.end(&mut runs);
    }
    runs
}

/// A fence's code among the runs, as they are put.
struct Code {
    /// The byte its fence starts at.
    fence: usize,
    /// Its runs with no highlight, which takes code's face if it has none of its own.
    plain: Vec<usize>,
    /// Its language gave it highlights of its own.
    own: bool,
}

impl Code {
    /// Its language is one oso does not read: the whole of it is in code's face.
    fn end(self, runs: &mut [Run]) {
        if !self.own {
            for at in self.plain {
                runs[at].1 = Some(Face::Reply(Style::Code).highlight());
            }
        }
    }
}

/// Puts `piece`'s runs: its own face, its language's highlights if it is code, and what the
/// parse says of the rest.
fn put(
    runs: &mut Vec<Run>,
    code: &mut Option<Code>,
    piece: &Piece<'_>,
    highlights: Option<&Highlights>,
    most: usize,
) {
    let own = piece.style.map(|style| Face::Reply(style).highlight());
    let len = piece.text.len();
    let (Some(from), Some(highlights)) = (piece.from, highlights) else {
        return runs.push((len, own));
    };
    if piece.code.is_none() && own.is_some() {
        return runs.push((len, own));
    }
    if let Some(fence) = piece.code
        && code.as_ref().is_none_or(|code| code.fence != fence)
        && let Some(before) = code.replace(Code {
            fence,
            plain: Vec::new(),
            own: false,
        })
    {
        before.end(runs);
    }
    for (range, highlight) in highlights.runs(from..from + len) {
        if runs.len() > most {
            return;
        }
        if let Some(code) = code.as_mut().filter(|_| piece.code.is_some()) {
            // A list's or a quote's own marks are no language's.
            let highlight = highlight.filter(|highlight| *highlight != Highlight::Markup);
            code.own |= highlight.is_some();
            if highlight.is_none() {
                code.plain.push(runs.len());
            }
            runs.push((range.len(), highlight));
        } else {
            runs.push((range.len(), highlight));
        }
    }
}

#[cfg(test)]
mod tests;
