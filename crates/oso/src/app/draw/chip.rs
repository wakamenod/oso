//! The cells a window fills behind a stretch of one of its rows, such as a key or a badge.
//!
//! Must not depend on: key presses, commands or what the text says.

use std::ops::Range;

use oso_core::frame::CellRect;
use oso_core::window::{Screen, ScreenRow};

/// The cells of `columns` of row `row`'s line, as `screen` shows it at `index` in the text area
/// `text`; none if none is in view.
pub(super) fn chip(
    text: CellRect,
    screen: &Screen,
    (index, row): (usize, &ScreenRow),
    columns: Range<usize>,
) -> Option<CellRect> {
    let origin = screen.origin(row);
    let shown = |column: usize| column.saturating_sub(origin).min(text.columns);
    let (from, to) = (shown(columns.start), shown(columns.end));
    (from < to).then(|| CellRect {
        row: text.row + index,
        rows: 1,
        column: text.column + from,
        columns: to - from,
    })
}
