//! A server's description of what is at point, in a float there (ADR 0138, Hover, step 10):
//! `C-c C-d` and `C-h .` show it, highlighted as Markdown with Rust in its fenced code, wrapped
//! to the float's width and scrolled by `C-v` and `M-v`.
//!
//! Must not depend on: `command` or `keymap`, or the host's servers, threads or I/O; the host
//! asks the server, and hands over the text the answer held.
//!
//! - The text goes in an internal buffer of its own, which no list shows and the agent channel
//!   never reads, in Markdown mode, or none for plain text, so the syntax worker highlights it
//!   and the display model boxes its controls, as any buffer's.
//! - The float is as wide as the text's longest line and as tall as its rows, at most
//!   [`COLUMNS_MAX`] by [`ROWS_MAX`], right below the cursor's row, or above it with more
//!   room there ([`frame::at_point`]).
//! - It is shown only while nothing moved since the ask: the window that asked is selected,
//!   shows the buffer at the revision asked about with its primary cursor where it was, and no
//!   prompt or isearch is open. Once anything moves it is gone.

use std::fmt;

use super::jumps::AskAt;
use crate::buffer::{Buffer, BufferId, Mode, Replacement};
use crate::display_map::wrap_map::WrapMap;
use crate::editor::Editor;
use crate::frame::{self, CellRect, FloatRegions};
use crate::window::{Rows, Screen, ScreenSource, Scroll, WindowId};

/// The most columns of text the float shows.
const COLUMNS_MAX: usize = 80;

/// The most rows of text the float shows.
const ROWS_MAX: usize = 20;

/// The name of the description's buffer, which no list shows.
const DESCRIBED: &str = "*lsp-description*";

/// The description shown at point: where it was asked, the buffer it is in, its width and the
/// row it shows first.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct HoverShown {
    window: WindowId,
    asked: BufferId,
    revision: u64,
    offset: usize,
    buffer: BufferId,
    columns: usize,
    top: usize,
}

impl fmt::Debug for HoverShown {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HoverShown")
            .field("top", &self.top)
            .finish_non_exhaustive()
    }
}

impl HoverShown {
    /// The buffer that holds the text.
    pub fn buffer(&self) -> BufferId {
        self.buffer
    }

    /// The first of its rows the float shows, from 0.
    pub fn top(&self) -> usize {
        self.top
    }
}

/// What the editor keeps of the description: what is shown, its buffer once made, and its
/// rows wrapped at the float's width for the buffer's revision.
#[derive(Default)]
pub(in crate::editor) struct Hover {
    shown: Option<HoverShown>,
    buffer: Option<BufferId>,
    wrap: Option<(u64, usize, WrapMap)>,
}

impl fmt::Debug for Hover {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Hover")
            .field("shown", &self.shown)
            .finish_non_exhaustive()
    }
}

impl Editor {
    /// Shows `text`, a server's description of `at`, Markdown if `markdown`, in a float at
    /// point, in place of any shown. Returns false, showing nothing, if anything moved since
    /// the ask, or the text is empty.
    pub fn show_hover(&mut self, at: AskAt, text: &str, markdown: bool) -> bool {
        let (window, buffer) = self.current();
        let still = window.buffer() == at.buffer
            && buffer.revision() == at.revision
            && window.selections.primary().cursor == at.offset
            && self.prompt().is_none()
            && self.isearch().is_none();
        if !still || text.trim().is_empty() {
            return false;
        }
        let window = window.id();
        let id = match self
            .servers
            .hover
            .buffer
            .filter(|&id| self.buffer(id).is_some())
        {
            Some(id) => id,
            None => {
                let id = self.add_buffer(Buffer::described(DESCRIBED));
                self.servers.hover.buffer = Some(id);
                id
            }
        };
        let mode = if markdown {
            Mode::Markdown
        } else {
            Mode::Fundamental
        };
        if !self.put_description(id, text, mode) {
            return false;
        }
        let columns = self.buffer(id).map_or(1, widest_line).clamp(1, COLUMNS_MAX);
        self.servers.hover.shown = Some(HoverShown {
            window,
            asked: at.buffer,
            revision: at.revision,
            offset: at.offset,
            buffer: id,
            columns,
            top: 0,
        });
        self.fit_hover();
        true
    }

    /// Puts `text` in the description's buffer `id`, in `mode`, in place of what it held.
    fn put_description(&mut self, id: BufferId, text: &str, mode: Mode) -> bool {
        let Some(buffer) = self.buffer_mut(id) else {
            return false;
        };
        buffer.set_internal_mode(mode);
        let edits = [Replacement {
            range: 0..buffer.text().len(),
            text,
        }];
        if let Some(staged) = buffer.stage_internal(&edits) {
            buffer.commit_internal(staged);
        }
        true
    }

    /// Closes the description, if one shows; its buffer lets its text go.
    pub fn close_hover(&mut self) {
        let Some(shown) = self.servers.hover.shown.take() else {
            return;
        };
        self.servers.hover.wrap = None;
        self.put_description(shown.buffer, "", Mode::Fundamental);
    }

    /// The description shown, while nothing moved since it was asked for.
    pub fn hover(&self) -> Option<&HoverShown> {
        let shown = self.servers.hover.shown.as_ref()?;
        let (window, buffer) = self.current();
        let still = window.id() == shown.window
            && window.buffer() == shown.asked
            && buffer.revision() == shown.revision
            && window.selections.primary().cursor == shown.offset
            && self.prompt().is_none()
            && self.isearch().is_none();
        still.then_some(shown)
    }

    /// Scrolls the description by `rows`, down for more than 0, stopping at its ends, as the
    /// wheel does; `C-v` and `M-v` scroll by its rows less two ([`Editor::hover_page`]). Says
    /// whether it moved.
    pub fn scroll_hover(&mut self, rows: isize) -> bool {
        let Some((regions, shown)) = self.hover_layout().zip(self.hover().copied()) else {
            return false;
        };
        let Some(buffer) = self.buffer(shown.buffer) else {
            return false;
        };
        let display = buffer.display();
        let wrap = self.hover_wrap(buffer.revision(), self.hover_columns(&shown));
        let count = Rows::new(&display, wrap).count();
        let last = count.saturating_sub(regions.inner.rows);
        let by = rows.unsigned_abs();
        let top = if rows < 0 {
            shown.top.saturating_sub(by)
        } else {
            shown.top.saturating_add(by).min(last)
        };
        if top == shown.top {
            return false;
        }
        if let Some(kept) = self.servers.hover.shown.as_mut() {
            kept.top = top;
        }
        self.fit_hover();
        true
    }

    /// The rows `C-v` and `M-v` scroll the description by: its float's, less two that stay in
    /// view, as a window's page keeps them. 0 while none shows.
    pub fn hover_page(&self) -> usize {
        self.hover_layout()
            .map_or(0, |regions| regions.inner.rows.saturating_sub(2).max(1))
    }

    /// Where the description's float goes, while one shows and the cursor is in view.
    pub fn hover_layout(&self) -> Option<FloatRegions> {
        let shown = self.hover()?;
        let buffer = self.buffer(shown.buffer)?;
        let display = buffer.display();
        let columns = self.hover_columns(shown);
        let rows = Rows::new(&display, self.hover_wrap(buffer.revision(), columns))
            .count()
            .clamp(1, ROWS_MAX);
        let anchor = self.cursor_cell()?;
        frame::at_point(
            self.selected_region(),
            (anchor.row, anchor.column),
            (rows, columns),
        )
    }

    /// The rows the description's float shows, into `out`; none while none shows.
    pub fn hover_screen(&self, out: &mut Screen) {
        let (Some(regions), Some(shown)) = (self.hover_layout(), self.hover()) else {
            out.clear();
            return;
        };
        let Some(buffer) = self.buffer(shown.buffer) else {
            out.clear();
            return;
        };
        let display = buffer.display();
        let rows = Rows::new(
            &display,
            self.hover_wrap(buffer.revision(), self.hover_columns(shown)),
        );
        let start = rows.start(shown.top);
        let source = ScreenSource {
            rows,
            scroll: Scroll { start, left: 0 },
            size: regions.inner.view_size(),
            cursor: start,
        };
        out.fill(source, &display, None);
    }

    /// The columns of text the float of `shown` gets: its text's widest line, cut to the
    /// selected window's text area less the float's padding.
    fn hover_columns(&self, shown: &HoverShown) -> usize {
        shown
            .columns
            .min(self.selected_region().columns.saturating_sub(2))
            .max(1)
    }

    /// The description's rows wrapped at `columns`, if they were for `revision`.
    fn hover_wrap(&self, revision: u64, columns: usize) -> Option<(&WrapMap, u64)> {
        let (kept, width, wrap) = self.servers.hover.wrap.as_ref()?;
        (*kept == revision && *width == columns).then_some((wrap, revision))
    }

    /// The columns its wrap is made for: one more than it shows, as a wrap leaves a column for
    /// a cursor at a row's end, and the float shows none.
    fn wrap_columns(columns: usize) -> usize {
        columns + 1
    }

    /// Brings the description's wrap in step with its text and width, after each command and
    /// each scroll: made anew for another text, and wrapped from the row shown first.
    pub(in crate::editor) fn fit_hover(&mut self) {
        let Some(shown) = self.servers.hover.shown else {
            return;
        };
        let Some(buffer) = self.buffer(shown.buffer) else {
            return;
        };
        let (display, revision) = (buffer.display(), buffer.revision());
        let columns = self.hover_columns(&shown);
        let kept = self.servers.hover.wrap.take();
        let (_, _, mut wrap) = kept
            .filter(|(kept, width, _)| *kept == revision && *width == columns)
            .unwrap_or_else(|| {
                let wrap = WrapMap::provisional(&display, Self::wrap_columns(columns));
                (revision, columns, wrap)
            });
        wrap.refill();
        let start = Rows::new(&display, Some((&wrap, revision))).start(shown.top);
        wrap.realize(&display, start, 2 * ROWS_MAX);
        self.servers.hover.wrap = Some((revision, columns, wrap));
    }

    /// The cell of the selected window's primary cursor, while it is in view.
    fn cursor_cell(&self) -> Option<CellRect> {
        let (window, buffer) = self.current();
        let display = buffer.display();
        let point = window.cursor_point(&display);
        let (row, origin) = window.cursor_place(&display, None)?;
        let region = self.selected_region();
        let column = point.column.checked_sub(origin)?;
        (row < region.rows && column < region.columns).then(|| CellRect {
            row: region.row + row,
            column: region.column + column,
            rows: 1,
            columns: 1,
        })
    }
}

/// The cells of the widest line of `buffer`'s text, as its display lays them out.
fn widest_line(buffer: &Buffer) -> usize {
    let (text, display) = (buffer.text(), buffer.display());
    (0..text.line_count())
        .map(|row| display.offset_to_display_point(text.line_end(row)).column)
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;
