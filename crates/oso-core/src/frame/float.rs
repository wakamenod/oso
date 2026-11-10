//! Where a float goes over the frame: a prompt's panel at the top of the tiled window, and a
//! completion's list centred in it, both on the window's gutter and text together, so that
//! line numbers never move one (ADR 0072, ADR 0075, ADR 0076, ADR 0097).
//!
//! Must not depend on: `editor`, `command`, or any GPU or window-system type.

use super::{CellRect, FrameLayout};

/// The cells a float takes over the frame (ADR 0072, ADR 0075).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FloatRegions {
    /// All of it: the cells its panel covers, padding included.
    pub outer: CellRect,
    /// What it shows, inside the padding: the label's rows, the gap and the answer's row.
    /// The same as `outer` where no padding fits.
    pub inner: CellRect,
    /// Whether a row of padding goes right above the answer's row: between the label and the
    /// answer, or at the top of a completion's float.
    pub gap: bool,
    /// The rows of candidates below the answer's padding, as wide as `inner` (ADR 0076), or
    /// its left 2/5 beside a preview (ADR 0088). Empty with none, or where the frame has no
    /// room for them.
    pub candidates: CellRect,
    /// Where the selected candidate's preview goes, right of the candidates past a column
    /// that divides them, in the candidates' rows (ADR 0088). Empty with none, or where the
    /// candidates would get fewer than [`MIN_LIST_COLUMNS`].
    pub preview: CellRect,
}

/// The fewest columns the candidates keep beside a preview; with fewer, the preview goes.
pub const MIN_LIST_COLUMNS: usize = 20;

impl FloatRegions {
    /// Whether a row of padding goes right below what it shows: below the answer, before any
    /// candidates.
    pub fn padded_below(&self) -> bool {
        let end = self.inner.row + self.inner.rows;
        if self.candidates.rows > 0 {
            self.candidates.row > end
        } else {
            self.outer.row + self.outer.rows > end
        }
    }

    /// Whether a row of padding goes below the candidates.
    pub fn candidates_padded(&self) -> bool {
        self.candidates.rows > 0
            && self.outer.row + self.outer.rows > self.candidates.row + self.candidates.rows
    }
}

/// The share of the frame's columns a float takes, in tenths, before its bounds.
const FLOAT_TENTHS: usize = 6;

/// The fewest columns a float takes, while the frame allows.
const FLOAT_MIN_COLUMNS: usize = 40;

/// The columns a float keeps from each edge of the frame.
const FLOAT_EDGE: usize = 2;

/// The columns of padding on each side of what a float shows, while it is wide enough.
const FLOAT_SIDE: usize = 2;

/// Where a float with `rows` rows of label goes, attached to the top edge of the window's
/// text area in `frame` and centered on its gutter and text, as the minibuffer does
/// (ADR 0026, ADR 0075, ADR 0097). It takes 60% of those columns, at least 40, and never
/// comes closer than 2 to an edge. Below the
/// label come a row of padding, the answer's row and a row of padding, with 2 columns of
/// padding on each side. A small frame gives up the padding below first, then the gap, then
/// the label's rows; with one row of text left, the label and the answer share it. It is
/// never taller than the text area, so the modeline and the echo area stay in view.
pub fn float_regions(frame: &FrameLayout, rows: usize) -> FloatRegions {
    answer_regions(frame, rows)
}

/// The share of the frame's columns a completion's float takes, in hundredths.
const COMPLETION_HUNDREDTHS: usize = 62;

/// Where a completion's float goes, centered in the window's gutter and text in `frame`
/// (ADR 0076, ADR 0097): the answer's row and `candidates` rows right below it, with no padding rows, as tall
/// whatever matches. It takes 62% of the columns, at least 40, and never comes closer than 2 to
/// an edge, with a column of padding at each side. A small frame gives up candidate rows. It is
/// never taller than the text area.
pub(crate) fn completion_regions(
    frame: &FrameLayout,
    candidates: usize,
    previews: bool,
) -> FloatRegions {
    let text = frame.window.area();
    let columns = float_columns(text.columns, COMPLETION_HUNDREDTHS, 100);
    let side = usize::from(columns >= 5);
    let shown = candidates.min(text.rows.saturating_sub(1));
    let rows = (1 + shown).min(text.rows);
    let outer = CellRect {
        row: text.row + (text.rows - rows) / 2,
        column: text.column + (text.columns - columns) / 2,
        rows,
        columns,
    };
    let inner = CellRect {
        rows: usize::from(rows > 0),
        column: outer.column + side,
        columns: columns - 2 * side,
        ..outer
    };
    let mut candidates = CellRect {
        row: inner.row + 1,
        rows: shown,
        ..inner
    };
    let mut preview = CellRect {
        rows: 0,
        columns: 0,
        ..candidates
    };
    // The list takes 2/5, a column divides, and the preview takes the rest.
    let list = inner.columns * 2 / 5;
    if previews && shown > 0 && list >= MIN_LIST_COLUMNS && inner.columns > list + 1 {
        candidates.columns = list;
        preview = CellRect {
            column: inner.column + list + 1,
            columns: inner.columns - list - 1,
            ..candidates
        };
    }
    FloatRegions {
        outer,
        inner,
        gap: false,
        candidates,
        preview,
    }
}

/// The columns of a float that takes `share` of `total` of the text area's `columns`, at least
/// 40, and never closer than 2 to an edge.
fn float_columns(columns: usize, share: usize, total: usize) -> usize {
    let room = columns.saturating_sub(2 * FLOAT_EDGE);
    if room == 0 {
        columns
    } else {
        (columns * share / total).max(FLOAT_MIN_COLUMNS).min(room)
    }
}

/// The columns of padding on each side of what a float of `columns` shows.
fn float_side(columns: usize) -> usize {
    if columns >= 12 {
        FLOAT_SIDE
    } else {
        usize::from(columns >= 5)
    }
}

/// [`float_regions`]: the label's rows, the gap, the answer and its padding.
fn answer_regions(frame: &FrameLayout, rows: usize) -> FloatRegions {
    let text = frame.window.area();
    let columns = float_columns(text.columns, FLOAT_TENTHS, 10);
    let side = float_side(columns);
    let rows = rows.max(1);
    let below = text.rows >= rows + 3;
    let gap = text.rows >= rows + 2;
    // With one row, or none, the label and the answer share what there is.
    let content = if text.rows <= 1 {
        text.rows
    } else {
        (rows + usize::from(gap) + 1).min(text.rows)
    };
    let outer = CellRect {
        row: text.row,
        column: text.column + (text.columns - columns) / 2,
        rows: content + usize::from(below),
        columns,
    };
    let inner = CellRect {
        column: outer.column + side,
        rows: content,
        columns: columns - 2 * side,
        ..outer
    };
    FloatRegions {
        outer,
        inner,
        gap,
        candidates: CellRect {
            rows: 0,
            columns: 0,
            ..inner
        },
        preview: CellRect {
            rows: 0,
            columns: 0,
            ..inner
        },
    }
}
