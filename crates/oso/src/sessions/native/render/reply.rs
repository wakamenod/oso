//! A reply's row (ADR 0165, "The look: Markdown"): its lines as `shown` reads its Markdown, each
//! after its indent, from where it grew or whole; its gap's blank line before them.
//!
//! Must not depend on: the theme, the UI or files.

use super::{Face, GUIDE, Rendered, blank_mark};
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
    if at.lines == 0 && row.gap {
        out.put("\n", None);
    }
    reply_lines(row, text, at, *streaming && !*cut, |mark, pieces| {
        out.put(mark, Some(Face::Dim));
        for piece in pieces {
            out.put(piece.text, piece.style.map(Face::Reply));
        }
        out.put("\n", None);
    });
    if *cut {
        out.put(
            &format!("{}… cut at {} MiB\n", reply_indent(row), MAX_TEXT >> 20),
            Some(Face::Dim),
        );
    }
    out
}

/// What each line of reply `row` starts with.
fn reply_indent(row: &Row) -> String {
    GUIDE.repeat(usize::from(row.depth)) + "  "
}

/// Gives `line` each line reply `row`, whose text is `text`, draws from `at` on: its indent, for
/// an empty line without the spaces at its end, and its pieces. An empty reply draws none.
pub(in crate::sessions::native) fn reply_lines<'a>(
    row: &Row,
    text: &'a str,
    at: LineStart,
    streams: bool,
    mut line: impl FnMut(&str, &[Piece<'a>]),
) {
    let indent = reply_indent(row);
    shown::walk(text, at, streams, |pieces| {
        line(blank_mark(&indent, pieces.is_empty()), pieces);
    });
}
