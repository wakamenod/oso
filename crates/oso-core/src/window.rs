//! Windows and scrolling (ADR 0007, ADR 0040).
//!
//! A window shows one buffer, with its own selections and scroll position.
//! Scrolling follows Emacs with `scroll-conservatively` at 101: when the
//! cursor would leave the window, the view moves by the fewest rows that
//! bring it back. `C-v` and `M-v` move by a screen, keeping
//! [`CONTEXT_LINES`] rows of context. Lines never wrap (ADR 0006), so a
//! long line scrolls sideways to keep the cursor in view, also by the
//! fewest columns.
//!
//! The scrolling rules are pure functions of positions and sizes, so they
//! are tested without a window.

use crate::buffer::BufferId;
use crate::display_map::DisplayPoint;
use crate::motion::Edge;
use crate::selection::SelectionSet;

/// Rows that `C-v` and `M-v` keep from the previous screen, as Emacs's
/// `next-screen-context-lines`.
pub const CONTEXT_LINES: usize = 2;

/// The first row and column a window shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Scroll {
    /// The display row at the top.
    pub top: usize,
    /// The display column at the left edge.
    pub left: usize,
}

/// How many rows and columns a window shows in full. Both at least 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ViewSize {
    pub rows: usize,
    pub columns: usize,
}

impl ViewSize {
    /// A size of `rows` by `columns`, each at least 1.
    pub fn new(rows: usize, columns: usize) -> Self {
        Self {
            rows: rows.max(1),
            columns: columns.max(1),
        }
    }
}

impl Default for ViewSize {
    /// 24 by 80, until the window reports its size.
    fn default() -> Self {
        Self::new(24, 80)
    }
}

/// A window: a view of one buffer.
#[derive(Clone, Debug)]
pub struct Window {
    buffer: BufferId,
    /// The cursors.
    pub selections: SelectionSet,
    /// What the window shows.
    pub scroll: Scroll,
    size: ViewSize,
}

impl Window {
    /// A window showing `buffer` from its start.
    pub fn new(buffer: BufferId) -> Self {
        Self {
            buffer,
            selections: SelectionSet::default(),
            scroll: Scroll::default(),
            size: ViewSize::default(),
        }
    }

    /// The buffer shown.
    pub fn buffer(&self) -> BufferId {
        self.buffer
    }

    /// Shows `buffer` from its start, with one cursor.
    pub(crate) fn show(&mut self, buffer: BufferId) {
        *self = Self {
            size: self.size,
            ..Self::new(buffer)
        };
    }

    /// The rows and columns shown in full.
    pub fn size(&self) -> ViewSize {
        self.size
    }

    pub(crate) fn set_size(&mut self, size: ViewSize) {
        self.size = size;
    }
}

/// The scroll position that shows the cursor at `cursor`, whose cluster
/// takes `cells` cells, moving `scroll` by as few rows and columns as
/// possible.
pub fn follow(scroll: Scroll, size: ViewSize, cursor: DisplayPoint, cells: usize) -> Scroll {
    let mut next = scroll;
    if cursor.row < scroll.top {
        next.top = cursor.row;
    } else if cursor.row >= scroll.top + size.rows {
        next.top = cursor.row + 1 - size.rows;
    }
    // A cluster wider than the window shows its start.
    let right = cursor.column + cells.min(size.columns);
    if cursor.column < scroll.left {
        next.left = cursor.column;
    } else if right > scroll.left + size.columns {
        next.left = right - size.columns;
    }
    next
}

/// The rows `C-v` and `M-v` move by default: a screen less
/// [`CONTEXT_LINES`], and at least 1.
pub fn screen_lines(size: ViewSize) -> usize {
    size.rows.saturating_sub(CONTEXT_LINES).max(1)
}

/// The top row after scrolling the text up by `lines` (`C-v`): the view
/// moves toward the end. Like Emacs, it fails with [`Edge::End`] when that
/// would move the top past the last row, and then does not move.
pub fn scroll_up(top: usize, row_count: usize, lines: usize) -> Result<usize, Edge> {
    let last = row_count.saturating_sub(1);
    match top.checked_add(lines) {
        Some(next) if next <= last && top < last => Ok(next),
        _ => Err(Edge::End),
    }
}

/// The top row after scrolling the text down by `lines` (`M-v`): the view
/// moves toward the start, stopping at the first row. Fails with
/// [`Edge::Start`] only when the first row is already at the top.
pub fn scroll_down(top: usize, lines: usize) -> Result<usize, Edge> {
    if top == 0 {
        Err(Edge::Start)
    } else {
        Ok(top.saturating_sub(lines))
    }
}

/// The top row after the mouse wheel or trackpad moved the view by `rows`
/// (down toward the end if positive). It stops at the first row, and
/// going down stops once the last row is at the bottom, though a view
/// already past that (after `C-v`) stays.
pub fn wheel(top: usize, size: ViewSize, row_count: usize, rows: isize) -> usize {
    if rows < 0 {
        return top.saturating_sub(rows.unsigned_abs());
    }
    let bottom = row_count.saturating_sub(size.rows).max(top);
    top.saturating_add(rows.unsigned_abs()).min(bottom)
}

/// The row a cursor on `row` moves to so that it stays in a view whose top
/// row is `top`, or `None` if it is already in view. As in Emacs, a cursor
/// above the view moves to its first row and one below to its last.
pub fn row_in_view(row: usize, top: usize, size: ViewSize, row_count: usize) -> Option<usize> {
    let bottom = (top + size.rows - 1).min(row_count.saturating_sub(1));
    if row < top {
        Some(top.min(bottom))
    } else if row > bottom {
        Some(bottom)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const SIZE: ViewSize = ViewSize {
        rows: 10,
        columns: 20,
    };

    fn at(row: usize, column: usize) -> DisplayPoint {
        DisplayPoint::new(row, column)
    }

    #[test]
    fn follow_scrolls_by_the_fewest_rows() {
        let scroll = Scroll { top: 5, left: 0 };
        assert_eq!(follow(scroll, SIZE, at(5, 0), 1), scroll);
        assert_eq!(follow(scroll, SIZE, at(14, 0), 1), scroll);
        assert_eq!(follow(scroll, SIZE, at(15, 0), 1).top, 6);
        assert_eq!(follow(scroll, SIZE, at(40, 0), 1).top, 31);
        assert_eq!(follow(scroll, SIZE, at(4, 0), 1).top, 4);
        assert_eq!(follow(scroll, SIZE, at(0, 0), 1).top, 0);
    }

    #[test]
    fn follow_scrolls_sideways_by_the_fewest_columns() {
        let scroll = Scroll { top: 0, left: 10 };
        assert_eq!(follow(scroll, SIZE, at(0, 10), 1), scroll);
        assert_eq!(follow(scroll, SIZE, at(0, 29), 1), scroll);
        assert_eq!(follow(scroll, SIZE, at(0, 30), 1).left, 11);
        // A wide character at the right edge shows both cells.
        assert_eq!(follow(scroll, SIZE, at(0, 29), 2).left, 11);
        assert_eq!(follow(scroll, SIZE, at(0, 3), 1).left, 3);
        // A cluster wider than the window shows its start.
        assert_eq!(follow(scroll, SIZE, at(0, 100), 64).left, 100);
    }

    #[test]
    fn screens_keep_two_rows_of_context() {
        assert_eq!(screen_lines(SIZE), 8);
        assert_eq!(screen_lines(ViewSize::new(2, 1)), 1);
        assert_eq!(screen_lines(ViewSize::new(1, 1)), 1);
    }

    #[test]
    fn scroll_up_stops_with_an_error_at_the_end() {
        assert_eq!(scroll_up(0, 100, 8), Ok(8));
        assert_eq!(scroll_up(88, 100, 8), Ok(96));
        assert_eq!(scroll_up(96, 100, 8), Err(Edge::End));
        assert_eq!(scroll_up(99, 100, 8), Err(Edge::End));
        assert_eq!(scroll_up(0, 5, 8), Err(Edge::End), "a short buffer");
        assert_eq!(scroll_up(0, 1, 1), Err(Edge::End));
        assert_eq!(
            scroll_up(usize::MAX - 1, usize::MAX, usize::MAX),
            Err(Edge::End)
        );
    }

    #[test]
    fn scroll_down_stops_at_the_start() {
        assert_eq!(scroll_down(20, 8), Ok(12));
        assert_eq!(scroll_down(3, 8), Ok(0));
        assert_eq!(scroll_down(0, 8), Err(Edge::Start));
    }

    #[test]
    fn the_wheel_stops_at_both_ends() {
        assert_eq!(wheel(0, SIZE, 100, -3), 0);
        assert_eq!(wheel(10, SIZE, 100, -3), 7);
        assert_eq!(wheel(10, SIZE, 100, 3), 13);
        assert_eq!(wheel(89, SIZE, 100, 3), 90, "last row at the bottom");
        assert_eq!(wheel(95, SIZE, 100, 3), 95, "past that after C-v");
        assert_eq!(wheel(95, SIZE, 100, -3), 92);
        assert_eq!(wheel(0, SIZE, 5, 3), 0, "a short buffer");
    }

    #[test]
    fn a_cursor_outside_the_view_moves_to_its_nearest_row() {
        assert_eq!(row_in_view(3, 10, SIZE, 100), Some(10));
        assert_eq!(row_in_view(12, 10, SIZE, 100), None);
        assert_eq!(row_in_view(25, 10, SIZE, 100), Some(19));
        // A view showing the end: its last row is the buffer's.
        assert_eq!(row_in_view(3, 95, SIZE, 100), Some(95));
        assert_eq!(row_in_view(99, 95, SIZE, 100), None);
    }

    proptest! {
        /// After following, the cursor's cell is in view, and a cursor
        /// already in view does not scroll.
        #[test]
        fn prop_follow_shows_the_cursor(
            top in 0usize..100, left in 0usize..100,
            row in 0usize..200, column in 0usize..200, cells in 1usize..3,
            rows in 1usize..30, columns in 1usize..30,
        ) {
            let size = ViewSize::new(rows, columns);
            let scroll = Scroll { top, left };
            let next = follow(scroll, size, at(row, column), cells);
            prop_assert!((next.top..next.top + rows).contains(&row));
            prop_assert!((next.left..next.left + columns).contains(&column));
            let visible = (top..top + rows).contains(&row)
                && column >= left && column + cells <= left + columns;
            if visible {
                prop_assert_eq!(next, scroll);
            }
            // The fewest rows: the cursor ends at an edge if it moved.
            if next.top != top {
                prop_assert!(row == next.top || row == next.top + rows - 1);
            }
        }

        /// C-v then M-v returns to the same top, away from the ends.
        #[test]
        fn prop_screens_round_trip(top in 0usize..1000, rows in 3usize..60) {
            let size = ViewSize::new(rows, 80);
            let lines = screen_lines(size);
            if let Ok(next) = scroll_up(top, 10_000, lines) {
                prop_assert_eq!(scroll_down(next, lines), Ok(top));
                // The last rows of the old screen are the first of the new.
                prop_assert_eq!(next + CONTEXT_LINES, top + rows);
            }
        }
    }
}
