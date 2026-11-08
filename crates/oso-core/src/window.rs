//! Windows and scrolling (ADR 0007, ADR 0040).
//!
//! Must not depend on: `editor`, `command` or any GPU or window-system type; a
//! window here is editor state, not an OS window.
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
use crate::display_map::{DisplayPoint, DisplaySnapshot};
use crate::motion::{self, Edge};
use crate::selection::SelectionSet;

mod places;

pub use places::Place;
pub(crate) use places::{Places, Walk};

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
    scroll: Scroll,
    size: ViewSize,
    /// Where it was in the buffers it showed before (ADR 0088).
    places: Places,
}

impl Window {
    /// A window showing `buffer` from its start.
    pub fn new(buffer: BufferId) -> Self {
        Self {
            buffer,
            selections: SelectionSet::default(),
            scroll: Scroll::default(),
            size: ViewSize::default(),
            places: Places::default(),
        }
    }

    /// The buffer shown.
    pub fn buffer(&self) -> BufferId {
        self.buffer
    }

    /// Shows `buffer`, keeping where the window was in the one it leaves. Gives where it was
    /// in `buffer` the last time, for the editor to clip to the text as it is now; until then it
    /// shows `buffer` from its start, with one cursor. Showing the buffer it shows starts it over
    /// there. Ends a walk of `M-,` and `M-/`.
    pub(crate) fn show(&mut self, buffer: BufferId) -> Option<Place> {
        let mut places = std::mem::take(&mut self.places);
        places.end_walk();
        if buffer != self.buffer {
            let place = Place {
                selections: std::mem::take(&mut self.selections),
                scroll: self.scroll,
            };
            places.leave(self.buffer, place);
        }
        let place = places.take(buffer);
        *self = Self {
            size: self.size,
            places,
            ..Self::new(buffer)
        };
        place
    }

    /// The buffers the window showed before the one it shows, most recent first, with where it
    /// was in each.
    pub fn places(&self) -> impl Iterator<Item = (BufferId, &Place)> {
        self.places.iter()
    }

    pub(crate) fn places_mut(&mut self) -> &mut Places {
        &mut self.places
    }

    pub(crate) fn walk(&self) -> Option<&Walk> {
        self.places.walk(self.buffer)
    }

    /// The rows and columns shown in full.
    pub fn size(&self) -> ViewSize {
        self.size
    }

    pub(crate) fn set_size(&mut self, size: ViewSize) {
        self.size = size;
    }

    /// What the window shows.
    pub fn scroll(&self) -> Scroll {
        self.scroll
    }

    /// Moves the view and leaves every cursor where it is, for following the
    /// cursor, for the sideways wheel and for a window that gets taller. Anything
    /// else that moves the view on its own uses [`Window::set_top`].
    pub(crate) fn set_scroll(&mut self, scroll: Scroll) {
        self.scroll = scroll;
    }

    /// Puts back cursors and a view that were saved together. The top row stays
    /// within the buffer, which may have changed since. The cursor may end up out
    /// of view, so `Editor::restore_positions` follows it afterwards.
    pub(crate) fn restore_positions(
        &mut self,
        selections: SelectionSet,
        scroll: Scroll,
        display: &DisplaySnapshot,
    ) {
        self.selections = selections;
        self.scroll = Scroll {
            top: scroll.top.min(display.row_count().saturating_sub(1)),
            left: scroll.left,
        };
    }

    /// The cursor a frame draws, and the view follows: the primary one.
    pub fn cursor_point(&self, display: &DisplaySnapshot) -> DisplayPoint {
        display.offset_to_display_point(self.selections.primary().cursor)
    }

    /// Shows rows from `top` on, or from the last row if that is past it, and
    /// moves every cursor the view left behind to the start of the nearest row in
    /// view, as Emacs does after `C-v`, `M-v` and the mouse wheel. The view follows
    /// the primary cursor sideways if it moved, so that it stays in view with
    /// `left` above 0. A cursor that did not move stays as it is, since the sideways
    /// wheel may have left it out of view on purpose. Returns whether the view moved.
    pub fn set_top(&mut self, top: usize, display: &DisplaySnapshot) -> bool {
        let row_count = display.row_count();
        let top = top.min(row_count.saturating_sub(1));
        if top == self.scroll.top {
            return false;
        }
        self.scroll.top = top;
        let size = self.size;
        let primary = self.selections.primary().cursor;
        self.selections.update(|selection| {
            let row = display.offset_to_display_point(selection.cursor).row;
            if let Some(row) = row_in_view(row, top, size, row_count) {
                selection.move_to(motion::line_start(display, row));
            }
            Ok::<(), ()>(())
        });
        if self.selections.primary().cursor != primary {
            let point = self.cursor_point(display);
            let cells = motion::cursor_cells(display, point);
            self.scroll = follow(self.scroll, size, point, cells);
        }
        true
    }
}

/// Whether a view whose top row is `top` reaches `row` or the rows after
/// it. A buffer that grew from `row` on needs a new frame only then.
pub fn reaches(top: usize, size: ViewSize, row: usize) -> bool {
    top.saturating_add(size.rows) > row
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

/// The left column after the sideways wheel or trackpad moved the view by `columns`
/// (toward the ends of lines if positive). It stops at the first column, and going
/// right stops once the end of the widest of the rows in view, `widest` cells, shows
/// with a cell for a cursor after it, though a view already past that stays.
pub(crate) fn wheel_sideways(left: usize, size: ViewSize, widest: usize, columns: isize) -> usize {
    if columns < 0 {
        return left.saturating_sub(columns.unsigned_abs());
    }
    let right = (widest + 1).saturating_sub(size.columns).max(left);
    left.saturating_add(columns.unsigned_abs()).min(right)
}

/// About how many columns take as much of the screen as a row: a cell is about half
/// as wide as it is high.
const COLUMNS_PER_ROW: f64 = 2.0;

/// Wheel and trackpad scrolling not yet worth a whole row or column, so that small
/// deltas add up. A wheel event counts along one axis, whichever it moved furthest
/// on, so that a gesture a little off straight stays on its axis. What the other
/// axis holds stays, so that a slow gesture keeps its progress, and one whose main
/// axis alternates still moves.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WheelRemainder {
    rows: f64,
    columns: f64,
}

impl WheelRemainder {
    /// Adds a delta in rows and columns, and takes out the whole ones.
    pub fn add(&mut self, rows: f64, columns: f64) -> (isize, isize) {
        if !rows.is_finite() || !columns.is_finite() {
            return (0, 0);
        }
        if rows.abs() * COLUMNS_PER_ROW >= columns.abs() {
            self.rows += rows;
            let whole = self.rows.trunc();
            self.rows -= whole;
            (whole as isize, 0)
        } else {
            self.columns += columns;
            let whole = self.columns.trunc();
            self.columns -= whole;
            (0, whole as isize)
        }
    }
}

/// Which row of the window `C-l` puts the cursor on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recenter {
    Middle,
    /// Counting from the top, or from the bottom if negative: -1 is the last.
    Line(isize),
}

/// The top row that puts a cursor on `row` where `target` says. The view stops at
/// the first row, so a cursor near it may stay above that place.
pub fn recenter_top(row: usize, size: ViewSize, target: Recenter) -> usize {
    let line = match target {
        Recenter::Middle => size.rows / 2,
        Recenter::Line(line) if line >= 0 => line.unsigned_abs().min(size.rows - 1),
        Recenter::Line(line) => size.rows.saturating_sub(line.unsigned_abs()),
    };
    row.saturating_sub(line)
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
    fn showing_a_buffer_keeps_the_window_size() {
        let mut window = Window::new(BufferId(1));
        window.set_size(SIZE);
        window.scroll.top = 3;
        window.show(BufferId(2));
        assert_eq!(window.buffer(), BufferId(2));
        assert_eq!(window.size(), SIZE);
        assert_eq!(window.scroll, Scroll::default());
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
        assert_eq!(scroll_up(99, 100, 0), Err(Edge::End), "already at the end");
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
    fn a_view_reaches_only_the_rows_it_shows_in_full() {
        assert!(reaches(10, SIZE, 15));
        assert!(reaches(10, SIZE, 19), "its last row");
        assert!(!reaches(10, SIZE, 20), "the row below it");
        assert!(reaches(30, SIZE, 21), "below the old end");
    }

    #[test]
    fn set_top_moves_cursors_left_behind() {
        use crate::display_map::{DisplayMap, TabWidth};
        let text: String = (0..30).map(|i| format!("{i:02}\n")).collect();
        let display = DisplayMap::new(&crate::rope::Rope::from(text.as_str()), TabWidth::default())
            .snapshot();
        let mut window = Window::new(BufferId(0));
        window.set_size(SIZE);
        window.selections = SelectionSet::new(5 * 3 + 1);
        assert!(window.set_top(3, &display));
        assert_eq!(
            window.selections.primary().cursor,
            5 * 3 + 1,
            "still in view"
        );
        assert!(window.set_top(8, &display));
        assert_eq!(
            window.selections.primary().cursor,
            8 * 3,
            "start of the top row"
        );
        assert!(window.set_top(0, &display));
        assert_eq!(window.selections.primary().cursor, 8 * 3);
        assert!(!window.set_top(0, &display));
    }

    fn numbered_rows(rows: usize) -> DisplaySnapshot {
        use crate::display_map::{DisplayMap, TabWidth};
        let text: String = (0..rows).map(|i| format!("{i:02}\n")).collect();
        DisplayMap::new(&crate::rope::Rope::from(text.as_str()), TabWidth::default()).snapshot()
    }

    #[test]
    fn set_top_stops_at_the_last_row() {
        let display = numbered_rows(30);
        let mut window = Window::new(BufferId(0));
        window.set_size(SIZE);
        assert!(window.set_top(usize::MAX, &display));
        assert_eq!(window.scroll.top, 30, "the empty row after the last line");
        assert_eq!(window.selections.primary().cursor, 30 * 3);
        assert!(!window.set_top(99, &display), "already there");
    }

    #[test]
    fn set_top_follows_a_primary_cursor_it_moves_with_the_view_sideways() {
        let display = numbered_rows(30);
        let mut window = Window::new(BufferId(0));
        window.set_size(SIZE);
        window.selections = SelectionSet::new(1);
        window.scroll.left = 1;
        assert!(window.set_top(5, &display));
        assert_eq!(window.selections.primary().cursor, 5 * 3, "at column 0");
        assert_eq!(window.scroll, Scroll { top: 5, left: 0 }, "and in view");
    }

    #[test]
    fn set_top_leaves_the_view_sideways_if_the_cursor_stays() {
        let display = numbered_rows(30);
        let mut window = Window::new(BufferId(0));
        window.set_size(SIZE);
        window.selections = SelectionSet::new(5 * 3);
        window.scroll.left = 7;
        assert!(window.set_top(3, &display));
        assert_eq!(window.scroll, Scroll { top: 3, left: 7 });
        assert_eq!(window.selections.primary().cursor, 5 * 3);
    }

    #[test]
    fn the_sideways_wheel_stops_at_both_ends() {
        assert_eq!(wheel_sideways(5, SIZE, 100, -2), 3);
        assert_eq!(wheel_sideways(1, SIZE, 100, -2), 0);
        assert_eq!(wheel_sideways(0, SIZE, 100, 6), 6);
        assert_eq!(wheel_sideways(80, SIZE, 100, 6), 81, "the widest row's end");
        assert_eq!(wheel_sideways(90, SIZE, 100, 6), 90, "past that stays");
        assert_eq!(wheel_sideways(90, SIZE, 100, -6), 84);
        assert_eq!(wheel_sideways(0, SIZE, 10, 6), 0, "rows that fit");
        assert_eq!(wheel_sideways(0, SIZE, 100, isize::MAX), 81);
        assert_eq!(wheel_sideways(5, SIZE, 100, isize::MIN), 0);
    }

    #[test]
    fn small_wheel_deltas_add_up_on_each_axis() {
        let mut wheel = WheelRemainder::default();
        assert_eq!(wheel.add(0.4, 0.0), (0, 0));
        assert_eq!(wheel.add(0.4, 0.0), (0, 0));
        assert_eq!(wheel.add(0.4, 0.0), (1, 0));
        assert_eq!(wheel.add(-3.5, 0.0), (-3, 0));
        assert_eq!(wheel.add(0.0, 2.5), (0, 2));
        assert_eq!(wheel.add(0.0, 0.5), (0, 1), "the half left over");
        assert_eq!(wheel.add(0.0, -7.0), (0, -7));
    }

    #[test]
    fn a_wheel_event_moves_along_its_main_axis_only() {
        let mut wheel = WheelRemainder::default();
        assert_eq!(wheel.add(0.1, 5.0), (0, 5), "sideways: the 0.1 goes");
        assert_eq!(wheel.add(2.0, 1.0), (2, 0), "down: the 1.0 goes");
        assert_eq!(wheel.add(1.0, 2.0), (1, 0), "as far on the screen: down");
        assert_eq!(wheel.add(1.0, 2.5), (0, 2), "further sideways");
        assert_eq!(wheel.add(0.0, 0.5), (0, 1), "the remainder adds up");
        assert_eq!(wheel.add(0.0, 0.0), (0, 0));
    }

    /// Pixels of 10 by 20 cells: a slow drag across, whose main axis takes turns.
    #[test]
    fn a_slow_gesture_whose_main_axis_alternates_still_moves() {
        let mut wheel = WheelRemainder::default();
        let (mut rows, mut columns) = (0, 0);
        for _ in 0..10 {
            for (x, y) in [(4.0, 3.0), (3.0, 4.0)] {
                let (down, across) = wheel.add(y / 20.0, x / 10.0);
                rows += down;
                columns += across;
            }
        }
        assert!(columns > 0, "it went sideways");
        assert!(rows > 0, "and down");
    }

    #[test]
    fn a_stray_sideways_event_keeps_a_slow_scrolls_progress() {
        let mut wheel = WheelRemainder::default();
        assert_eq!(wheel.add(0.6, 0.0), (0, 0));
        assert_eq!(wheel.add(0.0, 0.3), (0, 0), "a stray event");
        assert_eq!(wheel.add(0.6, 0.0), (1, 0), "nothing was lost");
    }

    #[test]
    fn a_bad_wheel_delta_is_ignored() {
        let mut wheel = WheelRemainder::default();
        assert_eq!(wheel.add(f64::NAN, 0.0), (0, 0));
        assert_eq!(wheel.add(0.0, f64::INFINITY), (0, 0));
        assert_eq!(wheel.add(1.5, 0.0), (1, 0));
    }

    #[test]
    fn recentering_puts_the_cursor_on_the_row_asked_for() {
        assert_eq!(recenter_top(50, SIZE, Recenter::Middle), 45);
        assert_eq!(recenter_top(50, SIZE, Recenter::Line(0)), 50);
        assert_eq!(recenter_top(50, SIZE, Recenter::Line(3)), 47);
        assert_eq!(recenter_top(50, SIZE, Recenter::Line(99)), 41);
        assert_eq!(recenter_top(50, SIZE, Recenter::Line(-1)), 41);
        assert_eq!(recenter_top(50, SIZE, Recenter::Line(-3)), 43);
        assert_eq!(recenter_top(50, SIZE, Recenter::Line(isize::MIN)), 50);
        assert_eq!(
            recenter_top(2, SIZE, Recenter::Line(-1)),
            0,
            "the first row"
        );
    }

    #[test]
    fn restoring_positions_keeps_the_cursors_and_the_top_row_in_the_buffer() {
        use crate::display_map::{DisplayMap, TabWidth};
        let text: String = (0..30).map(|i| format!("{i:02}\n")).collect();
        let display = DisplayMap::new(&crate::rope::Rope::from(text.as_str()), TabWidth::default())
            .snapshot();
        let mut window = Window::new(BufferId(0));
        window.set_size(SIZE);
        let scroll = Scroll { top: 20, left: 3 };
        window.restore_positions(SelectionSet::new(5), scroll, &display);
        assert_eq!(window.scroll(), scroll);
        assert_eq!(window.selections.primary().cursor, 5, "out of view is kept");
        let past_the_end = Scroll { top: 99, left: 3 };
        window.restore_positions(SelectionSet::new(5), past_the_end, &display);
        assert_eq!(window.scroll(), Scroll { top: 30, left: 3 });
    }

    #[test]
    fn a_cursor_outside_the_view_moves_to_its_nearest_row() {
        assert_eq!(row_in_view(3, 10, SIZE, 100), Some(10));
        assert_eq!(row_in_view(12, 10, SIZE, 100), None);
        assert_eq!(row_in_view(10, 10, SIZE, 100), None, "the first row");
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
