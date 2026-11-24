//! A native session's faces laid on its rows again (ADR 0165, part 2g): once highlighting is
//! on after it was off, which drops every buffer's highlights, the faces its rows were drawn
//! with, and those of what stands after them.
//!
//! Must not depend on: key presses or how a frame is drawn; `sync` says when.
//!
//! - In slices of [`SLICE_ROWS`] rows or [`SLICE_BYTES`], the newest first, one an event of
//!   every session, as a reply's Markdown is read again for each; the rows a window shows are
//!   laid with the first, or as it comes to show them.

use std::collections::HashSet;
use std::ops::Range;

use oso_core::buffer::BufferId;
use oso_core::editor::Editor;
use oso_core::syntax::Highlights;

use super::super::Native;
use super::{fits, row_range, set_highlights};
use crate::app::freer::Freer;
use crate::sessions::native::markdown::plain;
use crate::sessions::native::render;

/// The most rows whose faces are laid again in a slice.
pub(super) const SLICE_ROWS: usize = 512;

/// The bytes of rows past which a slice ends.
const SLICE_BYTES: usize = 128 << 10;

/// Which of a session's faces are missing.
#[derive(Debug, Default)]
pub(super) struct Faces {
    /// All of them: highlighting turned off after its rows were drawn (`Editor::set_font_lock`).
    lost: bool,
    /// They are being laid again: those of the rows before this one wait.
    laying: Option<usize>,
    /// The rows before it laid already, as a window showed them.
    laid: HashSet<usize>,
}

impl Faces {
    /// Whether any are, or may be, missing.
    pub(super) fn lost(&self) -> bool {
        self.lost || self.laying.is_some()
    }

    /// Highlighting turned off: the editor dropped them.
    pub(super) fn drop_all(&mut self) {
        *self = Self {
            lost: true,
            ..Self::default()
        };
    }
}

/// Lays the faces of `native`'s rows again as they were drawn, a reply's with no language's
/// highlights, in one change of its buffer's: a `slice` of them, the newest first, and those
/// in view, `shown` being room for what windows show; the rest at the next events.
/// The first time, of what stands after them too. A row whose text is not what it renders to now is left
/// with none. Whether a window shows the buffer, so that a frame is due, after which the next
/// event comes.
pub(super) fn lay_faces(
    editor: &mut Editor,
    freer: &Freer,
    native: &mut Native,
    shown: &mut Vec<(BufferId, Range<usize>)>,
    slice: bool,
) -> bool {
    let Some(buffer) = editor.buffer(native.buffer) else {
        return false;
    };
    let len = buffer.text().len();
    let first = std::mem::take(&mut native.markdown.faces.lost);
    // Rows drawn since highlighting is on again have theirs.
    let mut highlights = (buffer.highlights().cloned())
        .filter(|highlights| !first && highlights.len() == len)
        .unwrap_or_else(|| Highlights::new(len));
    let rows = native.rows.rows().len();
    let before = native.markdown.faces.laying.unwrap_or(rows).min(rows);
    let mut next = before;
    let end = native.placed.start(before);
    while slice
        && next > 0
        && before - next < SLICE_ROWS
        && end - native.placed.start(next) < SLICE_BYTES
    {
        next -= 1;
        if !native.markdown.faces.laid.remove(&next) {
            lay_row(native, next, &mut highlights);
        }
    }
    editor.shown_ranges(shown);
    for (_, range) in shown.iter().filter(|(id, _)| *id == native.buffer) {
        let start = native.placed.row_at(range.start).unwrap_or(rows);
        for row in start..next {
            if native.placed.start(row) >= range.end {
                break;
            }
            if native.markdown.faces.laid.insert(row) {
                lay_row(native, row, &mut highlights);
            }
        }
    }
    native.markdown.faces.laying = (next > 0).then_some(next);
    let tail = native.placed.end()..len;
    if first && tail.len() == native.tail.len() {
        highlights.replace(tail, native.tail.runs());
    }
    set_highlights(editor, freer, native.buffer, highlights)
}

/// Puts row `row`'s faces, as it is drawn, in `highlights`.
fn lay_row(native: &Native, row: usize, highlights: &mut Highlights) {
    let Some(at) = native.rows.rows().get(row) else {
        return;
    };
    let range = row_range(native, row);
    // A reply's text is not rendered again.
    let width = native.row_width(row);
    let runs = match plain(at, width) {
        Some(runs) => fits(&runs, &range).then_some(runs),
        None => {
            let rendered = render::render(at, native.rows.directory(), width);
            // Its runs leave out what has none at its end.
            (rendered.text.len() == range.len()).then(|| rendered.runs())
        }
    };
    if let Some(runs) = runs {
        highlights.replace(range, runs);
    }
}
