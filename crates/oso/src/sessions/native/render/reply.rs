//! A reply's row (ADR 0165, "The look: Markdown"): its lines as `shown` reads its Markdown, each
//! after its mark, from where it grew or whole.
//!
//! Must not depend on: the theme, the UI or files.

use super::{Face, Rendered, blank_mark};
use crate::sessions::native::rows::{Kind, Row};
use crate::sessions::native::shown::{self, LineStart, Piece};
use crate::sessions::native::wire::MAX_TEXT;

/// What a reply row draws from `at` on: what [`RowChange::Grew`](crate::sessions::native::rows::RowChange::Grew)
/// says to draw again, from the last line only. With what is before `at`, it draws what
/// [`render`](super::render) draws.
pub fn tail(row: &Row, at: LineStart) -> Rendered {
    let Kind::Reply {
        text,
        streaming,
        cut,
    } = &row.kind
    else {
        return Rendered::default();
    };
    // About what it draws, and a style every few words.
    let grown = text.len().saturating_sub(at.from);
    let mut out = Rendered {
        text: String::with_capacity(grown + grown / 8 + 16),
        faces: Vec::with_capacity(grown / 16 + 8),
        calls: Vec::new(),
    };
    reply_lines(row.depth, text, at, *streaming && !*cut, |mark, pieces| {
        out.put(mark, Some(Face::Dim));
        for piece in pieces {
            out.put(piece.text, piece.style.map(Face::Reply));
        }
        out.put("\n", None);
    });
    if *cut {
        out.put(
            &format!(
                "{}  … cut at {} MiB\n",
                "  ".repeat(usize::from(row.depth)),
                MAX_TEXT >> 20
            ),
            Some(Face::Dim),
        );
    }
    out
}

/// Gives `line` each line a reply of `text` at `depth` draws from `at` on: its mark, which is
/// the line's without its spaces at the end if it is empty, and its pieces. A reply that draws
/// nothing is its mark alone.
pub(in crate::sessions::native) fn reply_lines<'a>(
    depth: u8,
    text: &'a str,
    at: LineStart,
    streams: bool,
    mut line: impl FnMut(&str, &[Piece<'a>]),
) {
    let indent = "  ".repeat(usize::from(depth));
    let (first, follow) = (format!("{indent}● "), format!("{indent}  "));
    let mut lines = at.lines;
    shown::walk(text, at, streams, |pieces| {
        let before = if lines == 0 { &first } else { &follow };
        line(blank_mark(before, pieces.is_empty()), pieces);
        lines += 1;
    });
    if lines == 0 {
        line(blank_mark(&first, true), &[]);
    }
}
