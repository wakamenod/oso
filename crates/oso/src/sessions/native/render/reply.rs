//! A reply's row (ADR 0165, "The look: Markdown"): its lines as `shown` reads its Markdown, each
//! after its indent, from where it grew or whole; its gap's blank line before them.
//!
//! Must not depend on: the theme, the UI or files.

use super::{Face, GUIDE, Rendered, blank_mark};
use crate::sessions::native::rows::{Kind, Row};
use crate::sessions::native::shown::{self, LineStart, Piece};
use crate::sessions::native::wire::MAX_TEXT;

/// What a reply row draws from `at` on, for a window `width` cells wide: what [`RowChange::Grew`](crate::sessions::native::rows::RowChange::Grew)
/// says to draw again, from the last line only. With what is before `at`, it draws what
/// [`render`](super::render) draws.
pub fn tail(row: &Row, at: LineStart, width: usize) -> Rendered {
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
        ..Rendered::default()
    };
    if at.lines == 0 && row.gap {
        out.put("\n", None);
    }
    let streams = *streaming && !*cut;
    let (mut number, mut last) = (at.lines + usize::from(at.lines == 0 && row.gap), None);
    let end = reply_lines(row, text, at, streams, width, |mark, pieces, table| {
        out.reply_line(mark, pieces);
        if table.is_some() {
            out.table_line(number, table != last);
        }
        last = table;
        number += 1;
    });
    out.cells = end.table_cells().saturating_sub(at.table_cells());
    if *cut {
        out.put(
            &format!("{}… cut at {} MiB\n", reply_indent(row), MAX_TEXT >> 20),
            Some(Face::Dim),
        );
    }
    out
}

/// The lines of each whole table of reply `row`, drawn in a box or as written for a window `width`
/// cells wide, one after another: [`Rendered::tables`] says where in the row each is, and which
/// they are is the same at every width.
pub fn tables(row: &Row, width: usize) -> Rendered {
    let mut out = Rendered::default();
    let Kind::Reply {
        text,
        streaming,
        cut,
    } = &row.kind
    else {
        return out;
    };
    let indent = reply_indent(row);
    let (at, width) = (LineStart::default(), shown::table_width(row.depth, width));
    let (mut number, mut last) = (usize::from(row.gap), None);
    let end = shown::walk_tables(text, at, *streaming && !*cut, width, |pieces, table| {
        if table.is_some() {
            out.reply_line(&indent, pieces);
            out.table_line(number, table != last);
        }
        last = table;
        number += 1;
    });
    out.cells = end.table_cells();
    out
}

impl Rendered {
    /// A reply's line, after `mark`.
    fn reply_line(&mut self, mark: &str, pieces: &[Piece<'_>]) {
        self.put(mark, Some(Face::Dim));
        for piece in pieces {
            self.put(piece.text, piece.style.map(Face::Reply));
        }
        self.put("\n", None);
    }

    /// The row's line `number` is a table's, its first line if `first`.
    fn table_line(&mut self, number: usize, first: bool) {
        if first {
            self.tables.push(number..number);
        }
        if let Some(table) = self.tables.last_mut() {
            table.end = number + 1;
        }
    }
}

/// What each line of reply `row` starts with, which [`shown::table_width`] counts.
fn reply_indent(row: &Row) -> String {
    GUIDE.repeat(usize::from(row.depth)) + "  "
}

/// Gives `line` each line reply `row`, whose text is `text`, draws from `at` on, for a window
/// `width` cells wide: its indent, for an empty line without the spaces at its end, its pieces,
/// and the table it is of, if any; returns the mark past the last. An empty reply draws none.
pub(in crate::sessions::native) fn reply_lines(
    row: &Row,
    text: &str,
    at: LineStart,
    streams: bool,
    width: usize,
    mut line: impl FnMut(&str, &[Piece<'_>], Option<usize>),
) -> LineStart {
    let indent = reply_indent(row);
    let width = shown::table_width(row.depth, width);
    shown::walk(text, at, streams, width, |pieces, table| {
        line(blank_mark(&indent, pieces.is_empty()), pieces, table);
    })
}
