//! What a native session's rows draw for their window's width (ADR 0165, "The rows' look" and
//! "The look: tables"): its turns' results at the right edge, spaces before each, and the box of
//! each reply's table, for the narrowest window that shows the session, so that neither wraps in
//! any.
//!
//! Must not depend on: key presses or how a frame is drawn; `render` lays a row out for a
//! width, which this keeps.
//!
//! - A new row is drawn for the width kept. When the width changes, what is in view is drawn again
//!   at once, and what a reply that streams drew; then the newest [`NEWEST`] tables, or
//!   [`MOST_CELLS`] of them, [`SLICE_BYTES`] of their replies an event, those in view among them;
//!   the others as they come into view.
//! - A session's earlier turns are put in at once, so past [`MOST_CELLS`] a reply's tables
//!   are as written, as at no width, until they are drawn as above.
//! - Of a reply, only its tables' lines are drawn again, in one change, so the rest keeps its
//!   highlights.
//! - Only a session that a window shows is looked at.

use std::collections::BTreeMap;
use std::ops::Range;

use oso_core::buffer::BufferId;
use oso_core::editor::Editor;
use oso_core::rope::Rope;
use oso_core::syntax::Highlights;

use super::{Native, draw};
use crate::app::App;
use crate::sessions::native::render::{self, Rendered};
use crate::sessions::native::rows::{Kind, Row, RowChange};

/// How many of a session's tables, the newest, are drawn for a new width while out of view.
const NEWEST: usize = 256;

/// The bytes of replies read and drawn past which no more of them are in an event, and what a
/// reply costs at least, for its change of the buffer.
const SLICE_BYTES: usize = 64 << 10;
const REPLY_COST: usize = 2 << 10;

/// What tables take to draw, about in cells, past which those of earlier turns are put in as
/// written, and no more are drawn for a new width while out of view.
const MOST_CELLS: usize = 128 << 10;

/// The rows of a session that a window's width changes: its turns' results, and its replies with
/// a table.
#[derive(Debug, Default)]
pub(super) struct Widths {
    rows: BTreeMap<usize, Width>,
    /// The width changed, and the newest tables are not all drawn for it yet.
    changed: bool,
}

/// The width a row was drawn for last, the lines of its tables among its own, and what they took
/// to draw, about in cells.
#[derive(Clone, Debug)]
struct Width {
    width: usize,
    tables: Vec<Range<usize>>,
    cells: usize,
}

impl Widths {
    /// Row `row`, at `at`, was drawn for `width`, as `drawn` says, from where it `grew`, or whole.
    pub(super) fn drawn(
        &mut self,
        at: usize,
        row: &Row,
        drawn: &Rendered,
        grew: bool,
        width: usize,
    ) {
        if drawn.tables.is_empty() && !matches!(row.kind, Kind::Result(_)) {
            if !grew {
                self.rows.remove(&at);
            }
            return;
        }
        let before = self.rows.remove(&at).filter(|_| grew);
        let (mut tables, mut cells) = before.map_or((Vec::new(), 0), |row| (row.tables, row.cells));
        tables.extend(drawn.tables.iter().cloned());
        cells += drawn.cells;
        self.rows.insert(
            at,
            Width {
                width,
                tables,
                cells,
            },
        );
    }

    /// The width to draw `row`, of earlier turns, for, in a window of `width` once those before it
    /// took `cells`: none for a reply past the bound.
    pub(super) fn of_past(&mut self, row: &Row, cells: usize, width: usize) -> usize {
        if cells < MOST_CELLS || !matches!(row.kind, Kind::Reply { .. }) {
            return width;
        }
        self.changed = true;
        0
    }
}

impl Native {
    /// The width row `row` was drawn for.
    pub(super) fn row_width(&self, row: usize) -> usize {
        (self.widths.rows.get(&row)).map_or(self.rows.width(), |row| row.width)
    }
}

impl App {
    /// After each event: what was drawn for another width than its window's is drawn again.
    pub(super) fn sync_native_widths(&mut self) {
        let natives = &self.hosting.natives;
        let shows = |id| {
            (natives.map.values().chain(natives.aside.values())).any(|native| native.buffer == id)
        };
        if !(self.editor.windows()).any(|window| shows(window.buffer())) {
            return;
        }
        let mut shown = Vec::new();
        self.editor.shown_ranges(&mut shown);
        let (mut redraw, mut slice) = (false, SLICE_BYTES);
        let natives = &mut self.hosting.natives;
        for native in natives.map.values_mut().chain(natives.aside.values_mut()) {
            redraw |= lay(&mut self.editor, native, &shown, &mut slice);
        }
        if redraw {
            self.request_redraw();
        }
    }
}

/// Keeps the width of the narrowest window that shows `native`, and draws again what was drawn
/// for another: in view, as `shown` says, then of the newest tables until `slice` bytes are.
/// Whether any was, so that a frame is due, after which the next event comes.
fn lay(
    editor: &mut Editor,
    native: &mut Native,
    shown: &[(BufferId, Range<usize>)],
    slice: &mut usize,
) -> bool {
    let width = (editor.windows())
        .filter(|window| window.buffer() == native.buffer)
        .filter_map(|window| editor.regions(window.id()))
        // The last column is the wrap mark's.
        .map(|regions| regions.text.columns.saturating_sub(1))
        .min();
    let Some(width) = width else {
        return false;
    };
    let mut stale = Vec::new();
    if width != native.rows.width() {
        stale = native.rows.set_width(width);
        native.widths.changed = true;
    }
    for (_, range) in shown.iter().filter(|(id, _)| *id == native.buffer) {
        let Some(first) = native.placed.row_at(range.start) else {
            continue;
        };
        let last = (native.placed.row_at(range.end)).unwrap_or(native.rows.rows().len());
        let rows = native.widths.rows.range(first..=last.max(first));
        stale.extend(
            rows.filter(|(_, row)| row.width != width)
                .map(|(at, _)| *at),
        );
    }
    // Several windows may show a row, or one that streams.
    stale.sort_unstable();
    stale.dedup();
    let changed = native.widths.changed;
    if !stale.is_empty() || changed {
        // The rows in view wrap within a budget an event.
        editor.refill_wraps();
    }
    for row in &stale {
        *slice = slice.saturating_sub(lay_row(editor, native, *row, width));
    }
    let mut any = !stale.is_empty();
    if changed {
        let (laid, done) = lay_newest(editor, native, width, slice);
        native.widths.changed = !done;
        any |= laid || !done;
    }
    any
}

/// Draws again for `width` the replies of the newest [`NEWEST`] tables, or [`MOST_CELLS`] of them,
/// until `slice` bytes are read and drawn. Whether any was, and
/// whether none is left.
fn lay_newest(
    editor: &mut Editor,
    native: &mut Native,
    width: usize,
    slice: &mut usize,
) -> (bool, bool) {
    let (mut tables, mut cells, mut any) = (0, 0, false);
    let mut before = usize::MAX;
    // What a row takes is known once it is drawn.
    while tables < NEWEST && cells < MOST_CELLS {
        let older = native.widths.rows.range(..before).next_back();
        let Some((at, row)) = older.map(|(at, row)| (*at, row)) else {
            break;
        };
        before = at;
        if row.width != width && !row.tables.is_empty() {
            if *slice == 0 {
                return (any, false);
            }
            any = true;
            *slice = slice.saturating_sub(lay_row(editor, native, at, width));
        }
        if let Some(row) = native.widths.rows.get(&at) {
            tables += row.tables.len();
            cells += row.cells;
        }
    }
    (any, true)
}

/// Draws row `row` for `width`: of a reply, its tables' lines alone. About the bytes read and
/// drawn.
fn lay_row(editor: &mut Editor, native: &mut Native, row: usize, width: usize) -> usize {
    let Some(of) = native.rows.rows().get(row) else {
        return 0;
    };
    let Kind::Reply { text, .. } = &of.kind else {
        draw(editor, native, [RowChange::Changed(row)]);
        return 0;
    };
    let after = render::tables(of, width);
    let cost = REPLY_COST + text.len() + after.text.len();
    if let Some((range, length)) = put_tables(editor, native, row, &after) {
        native.placed.put(row, range, length);
    }
    let (tables, cells) = (after.tables, after.cells);
    native.widths.rows.insert(
        row,
        Width {
            width,
            tables,
            cells,
        },
    );
    cost
}

/// Puts `after`, the tables of reply `row`, in place of those it drew, in one change from the first
/// that differs to the last; the lines between keep their highlights. The bytes replaced and
/// what took their place, if any differs.
fn put_tables(
    editor: &mut Editor,
    native: &Native,
    row: usize,
    after: &Rendered,
) -> Option<(Range<usize>, usize)> {
    let buffer = editor.buffer(native.buffer)?;
    let text = buffer.text();
    let before = &native.widths.rows.get(&row)?.tables;
    debug_assert_eq!(before.len(), after.tables.len());
    let first = text.offset_to_point(native.placed.start(row)).row;
    let mut faces = Highlights::new(after.text.len());
    faces.replace(0..after.text.len(), after.runs());
    let (mut put, mut runs) = (String::new(), Vec::new());
    let mut whole: Option<Range<usize>> = None;
    let mut at = 0;
    for (before, after_lines) in before.iter().zip(&after.tables) {
        let start = at;
        for _ in after_lines.clone() {
            at += after.text[at..].find('\n').map_or(0, |end| end + 1);
        }
        let drawn = &after.text[start..at];
        let range = text.line_start(first + before.start)..text.line_start(first + before.end);
        if same(text, range.clone(), drawn) {
            continue;
        }
        let kept = whole.as_ref().map_or(range.start, |whole| whole.end)..range.start;
        // The lines between two tables, as they are.
        for chunk in text.chunks_in_range(kept.clone()) {
            put.push_str(chunk);
        }
        if let Some(highlights) = buffer.highlights() {
            let between = highlights.runs(kept);
            runs.extend(between.map(|(range, highlight)| (range.len(), highlight)));
        }
        put.push_str(drawn);
        runs.extend(
            faces
                .runs(start..at)
                .map(|(range, highlight)| (range.len(), highlight)),
        );
        whole = Some(whole.map_or(range.start, |whole| whole.start)..range.end);
    }
    let range = whole?;
    editor.replace_native_rows(native.buffer, range.clone(), &put, &runs);
    Some((range, put.len()))
}

/// Whether `range` of `text` is `drawn`.
fn same(text: &Rope, range: Range<usize>, drawn: &str) -> bool {
    let mut rest = drawn;
    range.len() == drawn.len()
        && (text.chunks_in_range(range)).all(|chunk| match rest.strip_prefix(chunk) {
            Some(after) => {
                rest = after;
                true
            }
            None => false,
        })
}

#[cfg(test)]
mod tests;
