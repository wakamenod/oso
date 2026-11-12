//! Windows and scrolling (ADR 0007, ADR 0040, ADR 0100, ADR 0103).
//!
//! Must not depend on: `editor`, `command` or any GPU or window-system type; a
//! window here is editor state, not an OS window.
//!
//! A window shows one buffer, with its own selections and scroll position.
//! Scrolling follows Emacs with `scroll-conservatively` at 101: when the
//! cursor would leave the window, the view moves by the fewest rows that
//! bring it back. `C-v` and `M-v` move by a screen, keeping
//! [`CONTEXT_LINES`] rows of context. A long line that does not wrap scrolls
//! sideways to keep the cursor in view, also by the fewest columns.
//!
//! Rows here are view rows: a wrapped buffer's from the window's [`WrapMap`], at its
//! width, and otherwise one a logical line. The window keeps its top as an offset, which
//! follows edits as a marker, and shows from the start of the view row that holds it,
//! however the rows change around it, so that what it shows stays put (ADR 0103).
//!
//! The scrolling rules are pure functions of rows and sizes, so they
//! are tested without a window.

use std::fmt;

use crate::buffer::BufferId;
use crate::display_map::wrap_map::{WrapMap, WrapSnapshot};
use crate::display_map::{DisplayPoint, DisplaySnapshot};
use crate::motion::{self, Edge};
use crate::selection::SelectionSet;

mod places;
mod rows;
mod screen;

pub use places::Place;
pub(crate) use places::{Places, Walk};
pub(crate) use rows::Rows;
pub(crate) use screen::ScreenSource;
pub use screen::{Screen, ScreenRow};

/// Rows that `C-v` and `M-v` keep from the previous screen, as Emacs's
/// `next-screen-context-lines`.
pub const CONTEXT_LINES: usize = 2;

/// What a window shows, as it keeps it and as a place keeps it (ADR 0100).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Scroll {
    /// The offset its first view row holds: where that row starts, once the view is
    /// scrolled, and through edits, rewraps and reports wherever its text went.
    pub start: usize,
    /// The display column at the left edge; 0 while its buffer wraps.
    pub left: usize,
}

/// The first row a window shows and its left column, in view rows, as the rules of scrolling
/// count it. What a frame draws from it is a [`Screen`] (ADR 0104).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ViewTop {
    pub row: usize,
    pub left: usize,
}

/// A window's name for as long as the editor runs. The editor gives each window a new one and
/// never gives one again, so state that names a window cannot name another once it is
/// deleted (ADR 0112).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WindowId(u64);

impl WindowId {
    /// The id numbered `n`.
    pub(crate) const fn new(n: u64) -> Self {
        Self(n)
    }

    /// Its number, by which the host names what it keeps for the window, as its panes.
    pub const fn get(self) -> u64 {
        self.0
    }
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
pub struct Window {
    id: WindowId,
    buffer: BufferId,
    /// The cursors.
    pub selections: SelectionSet,
    /// What the window shows.
    scroll: Scroll,
    size: ViewSize,
    /// Where it was in the buffers it showed before (ADR 0088).
    places: Places,
    /// The rows of its buffer at its width, while the buffer wraps (ADR 0100).
    wrap: Option<WrapMap>,
    /// The buffer's revision the wrap follows.
    wrap_revision: u64,
    /// The widest its gutter has been for the buffer it shows (ADR 0104).
    gutter_floor: Option<(BufferId, usize)>,
    /// The buffers it was on before each internal buffer it has gone to, the first first.
    /// Where it was in each is in its places (ADR 0088).
    way_back: Vec<BufferId>,
}

#[cfg(test)]
impl Clone for Window {
    /// A window like this one, whose wrap has a generation of its own, so that no job of this
    /// window reports to it. It keeps the id, so it stands for the same window, as tests
    /// compare it; a split makes its new window with [`Window::split_off`] (ADR 0112).
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            selections: self.selections.clone(),
            ..self.split_off(self.id)
        }
    }
}

impl fmt::Debug for Window {
    /// Leaves out the offsets: where rows break tells how long words are in a buffer that
    /// may hold secrets (ADR 0100).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Window")
            .field("id", &self.id)
            .field("buffer", &self.buffer)
            .field("size", &self.size)
            .field("wrapped", &self.wrap.is_some())
            .finish_non_exhaustive()
    }
}

impl Window {
    /// A window named `id` showing `buffer` from its start.
    pub(crate) fn new(id: WindowId, buffer: BufferId) -> Self {
        Self {
            id,
            buffer,
            selections: SelectionSet::default(),
            scroll: Scroll::default(),
            size: ViewSize::default(),
            places: Places::default(),
            wrap: None,
            wrap_revision: 0,
            gutter_floor: None,
            way_back: Vec::new(),
        }
    }

    /// A new window named `id` that shows what this one does, as a split makes it (ADR 0112):
    /// the same buffer, the primary cursor with its goal and mark, the mark ring and whether
    /// the region shows, but none of the other cursors; the same view, places, gutter and way
    /// back. Its wrap has a generation of its own, so no job of this window reports to it, and
    /// is wrapped again at its own width (ADR 0103).
    pub(crate) fn split_off(&self, id: WindowId) -> Self {
        let mut selections = self.selections.clone();
        selections.keep_primary();
        Self {
            id,
            buffer: self.buffer,
            selections,
            scroll: self.scroll,
            size: self.size,
            places: self.places.clone(),
            wrap: self.wrap.as_ref().map(WrapMap::detached),
            wrap_revision: self.wrap_revision,
            gutter_floor: self.gutter_floor,
            way_back: self.way_back.clone(),
        }
    }

    /// Its name.
    pub fn id(&self) -> WindowId {
        self.id
    }

    /// The buffer shown.
    pub fn buffer(&self) -> BufferId {
        self.buffer
    }

    /// The widest its gutter has been for the buffer it shows, as the layout last gave it.
    pub(crate) fn gutter_floor(&self) -> Option<(BufferId, usize)> {
        self.gutter_floor
    }

    pub(crate) fn set_gutter_floor(&mut self, floor: Option<(BufferId, usize)>) {
        self.gutter_floor = floor;
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

    /// Its rows of its buffer at its width while the buffer wraps, some perhaps provisional
    /// until a job reports.
    pub fn wrap(&self) -> Option<&WrapSnapshot> {
        self.wrap.as_ref().map(WrapMap::snapshot)
    }

    /// Its rows of `display`, its buffer as it shows now, while the buffer wraps: none while
    /// they are of another text, as until the editor brings them along, when a frame shows
    /// one row a logical line.
    pub fn wrap_of(&self, display: &DisplaySnapshot) -> Option<&WrapSnapshot> {
        let wrap = self.wrap.as_ref()?;
        rows::follows(wrap, self.wrap_revision, display)
    }

    pub(crate) fn wrap_map(&self) -> Option<&WrapMap> {
        self.wrap.as_ref()
    }

    pub(crate) fn wrap_map_mut(&mut self) -> Option<&mut WrapMap> {
        self.wrap.as_mut()
    }

    /// Puts in `wrap`, the rows of its buffer as `display` shows it at `revision`, or none.
    /// While wrapped, the left column is 0.
    pub(crate) fn set_wrap(
        &mut self,
        wrap: Option<WrapMap>,
        display: &DisplaySnapshot,
        revision: u64,
    ) {
        self.wrap = wrap;
        self.wrap_revision = revision;
        if self.wrap.is_some() {
            if self.rows(display).wrapped() {
                self.scroll.left = 0;
            }
            self.realize_top(display);
        }
    }

    /// The revision of its buffer that its wrap follows.
    pub(crate) fn wrap_revision(&self) -> u64 {
        self.wrap_revision
    }

    /// Notes that its wrap follows its buffer at `revision`.
    pub(crate) fn set_wrap_revision(&mut self, revision: u64) {
        self.wrap_revision = revision;
    }

    /// Puts in the wrap and the view that followed a change of its buffer, which is now at
    /// `revision`. Nothing here can panic (ADR 0053).
    pub(crate) fn follow_change(&mut self, wrap: Option<WrapMap>, scroll: Scroll, revision: u64) {
        self.wrap = wrap;
        self.wrap_revision = revision;
        self.scroll = scroll;
    }

    /// Its view rows over `display`, its buffer as it shows now.
    pub(crate) fn rows<'a>(&'a self, display: &'a DisplaySnapshot) -> Rows<'a> {
        Rows::new(
            display,
            self.wrap.as_ref().map(|wrap| (wrap, self.wrap_revision)),
        )
    }

    /// How many view rows its buffer has.
    pub fn row_count(&self, display: &DisplaySnapshot) -> usize {
        self.rows(display).count()
    }

    /// The view row at the top.
    pub fn top_row(&self, display: &DisplaySnapshot) -> usize {
        self.rows(display).row_of(self.scroll.start)
    }

    /// The bytes its rows in view hold, from its top row's start to where the row after its
    /// last starts: a few seeks, with no row laid out (ADR 0111).
    pub fn shown_bytes(&self, display: &DisplaySnapshot) -> std::ops::Range<usize> {
        let rows = self.rows(display);
        let top = rows.row_of(self.scroll.start);
        let after = top.saturating_add(self.size.rows);
        let end = if after < rows.count() {
            rows.start(after)
        } else {
            display.len()
        };
        rows.start(top)..end
    }

    /// The view row of the primary cursor.
    pub fn cursor_row(&self, display: &DisplaySnapshot) -> usize {
        self.rows(display).row_of(self.selections.primary().cursor)
    }

    /// The top and the left column as rows count them.
    fn view_top(&self, display: &DisplaySnapshot) -> ViewTop {
        ViewTop {
            row: self.top_row(display),
            left: self.scroll.left,
        }
    }

    /// Moves the view and leaves every cursor where it is, for following the
    /// cursor, for the sideways wheel and for a window that gets taller. Anything
    /// else that moves the view on its own uses [`Window::set_top`].
    pub(crate) fn set_scroll(&mut self, scroll: Scroll) {
        self.scroll = scroll;
    }

    /// Shows `top` as rows count them, its row's start at the top, and leaves every cursor
    /// where it is. Returns whether the view moved.
    pub(crate) fn set_view_top(&mut self, top: ViewTop, display: &DisplaySnapshot) -> bool {
        let rows = self.rows(display);
        let scroll = Scroll {
            start: rows.start(top.row),
            left: if rows.wrapped() { 0 } else { top.left },
        };
        let moved = scroll != self.scroll;
        self.scroll = scroll;
        moved
    }

    /// Wraps the row that holds the top if it is provisional, so that the view does not
    /// show from the start of its logical line (ADR 0103).
    pub(crate) fn realize_top(&mut self, display: &DisplaySnapshot) {
        if let Some(wrap) = &mut self.wrap {
            wrap.realize(display, self.scroll.start, 0);
        }
    }

    /// Wraps at once the provisional rows from the one that holds `at` to `rows` past it,
    /// within what is left of the command's budget (ADR 0103). Returns whether they changed,
    /// after which the top's and the cursor's rows are found again.
    pub(crate) fn realize(&mut self, display: &DisplaySnapshot, at: usize, rows: usize) -> bool {
        self.wrap
            .as_mut()
            .is_some_and(|wrap| wrap.realize(display, at, rows))
    }

    /// Wraps at once the provisional rows in view, and as many again below them, which
    /// `C-v` and the wheel reach. Returns whether they changed.
    pub(crate) fn realize_view(&mut self, display: &DisplaySnapshot) -> bool {
        let (start, rows) = (self.scroll.start, self.size.rows);
        self.realize(display, start, rows.saturating_mul(2))
    }

    /// Puts back cursors and a view that were saved together. The top stays within the
    /// buffer, which may have changed since. The cursor may end up out of view, and rows
    /// there may be provisional, so the editor follows it afterwards, which wraps them.
    pub(crate) fn restore_positions(
        &mut self,
        selections: SelectionSet,
        scroll: Scroll,
        display: &DisplaySnapshot,
    ) {
        self.selections = selections;
        self.scroll = Scroll {
            start: scroll.start.min(display.len()),
            left: if self.wrap.is_some() { 0 } else { scroll.left },
        };
    }

    /// The cursor a frame draws, and the view follows: the primary one.
    pub fn cursor_point(&self, display: &DisplaySnapshot) -> DisplayPoint {
        display.offset_to_display_point(self.selections.primary().cursor)
    }

    /// Shows view rows from `top` on, or from the last row if that is past it, and
    /// moves every cursor the view left behind to the start of the nearest row in
    /// view, as Emacs does after `C-v`, `M-v` and the mouse wheel. The view follows
    /// the primary cursor sideways if it moved, so that it stays in view with
    /// `left` above 0. A cursor that did not move stays as it is, since the sideways
    /// wheel may have left it out of view on purpose. Returns whether the view moved.
    pub fn set_top(&mut self, top: usize, display: &DisplaySnapshot) -> bool {
        let rows = Rows::new(
            display,
            self.wrap.as_ref().map(|wrap| (wrap, self.wrap_revision)),
        );
        let row_count = rows.count();
        let top = top.min(row_count.saturating_sub(1));
        if top == rows.row_of(self.scroll.start) {
            return false;
        }
        self.scroll.start = rows.start(top);
        let size = self.size;
        let primary = self.selections.primary().cursor;
        self.selections.update(|selection| {
            let row = rows.row_of(selection.cursor);
            if let Some(row) = row_in_view(row, top, size, row_count) {
                selection.move_to(rows.start(row));
            }
            Ok::<(), ()>(())
        });
        if self.selections.primary().cursor != primary && !rows.wrapped() {
            let point = self.cursor_point(display);
            let cells = motion::cursor_cells(display, point);
            let next = follow(self.view_top(display), size, point, cells);
            self.scroll.left = next.left;
        }
        true
    }
}

/// Whether a view whose top row is `top` reaches `row` or the rows after
/// it. A buffer that grew from `row` on needs a new frame only then.
pub fn reaches(top: usize, size: ViewSize, row: usize) -> bool {
    top.saturating_add(size.rows) > row
}

/// The top that shows the cursor at `cursor`, a view row and a column, whose cluster
/// takes `cells` cells, moving `scroll` by as few rows and columns as possible.
pub fn follow(scroll: ViewTop, size: ViewSize, cursor: DisplayPoint, cells: usize) -> ViewTop {
    let mut next = scroll;
    if cursor.row < scroll.row {
        next.row = cursor.row;
    } else if cursor.row >= scroll.row + size.rows {
        next.row = cursor.row + 1 - size.rows;
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
        let mut window = Window::new(WindowId(0), BufferId(1));
        window.set_size(SIZE);
        window.scroll.start = 9;
        window.show(BufferId(2));
        assert_eq!(window.buffer(), BufferId(2));
        assert_eq!(window.size(), SIZE);
        assert_eq!(window.scroll, Scroll::default());
    }

    #[test]
    fn debug_shows_no_offset() {
        let mut window = Window::new(WindowId(0), BufferId(1));
        window.selections = SelectionSet::new(1234);
        window.scroll.start = 5678;
        let shown = format!("{window:?}");
        assert!(
            !shown.contains("1234") && !shown.contains("5678"),
            "{shown}"
        );
    }

    #[test]
    fn follow_scrolls_by_the_fewest_rows() {
        let scroll = ViewTop { row: 5, left: 0 };
        assert_eq!(follow(scroll, SIZE, at(5, 0), 1), scroll);
        assert_eq!(follow(scroll, SIZE, at(14, 0), 1), scroll);
        assert_eq!(follow(scroll, SIZE, at(15, 0), 1).row, 6);
        assert_eq!(follow(scroll, SIZE, at(40, 0), 1).row, 31);
        assert_eq!(follow(scroll, SIZE, at(4, 0), 1).row, 4);
        assert_eq!(follow(scroll, SIZE, at(0, 0), 1).row, 0);
    }

    #[test]
    fn follow_scrolls_sideways_by_the_fewest_columns() {
        let scroll = ViewTop { row: 0, left: 10 };
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
        let mut window = Window::new(WindowId(0), BufferId(0));
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
        let mut window = Window::new(WindowId(0), BufferId(0));
        window.set_size(SIZE);
        assert!(window.set_top(usize::MAX, &display));
        assert_eq!(
            window.scroll.start,
            30 * 3,
            "the empty row after the last line"
        );
        assert_eq!(window.selections.primary().cursor, 30 * 3);
        assert!(!window.set_top(99, &display), "already there");
    }

    #[test]
    fn set_top_follows_a_primary_cursor_it_moves_with_the_view_sideways() {
        let display = numbered_rows(30);
        let mut window = Window::new(WindowId(0), BufferId(0));
        window.set_size(SIZE);
        window.selections = SelectionSet::new(1);
        window.scroll.left = 1;
        assert!(window.set_top(5, &display));
        assert_eq!(window.selections.primary().cursor, 5 * 3, "at column 0");
        assert_eq!(
            window.scroll,
            Scroll {
                start: 5 * 3,
                left: 0
            },
            "and in view"
        );
    }

    #[test]
    fn set_top_leaves_the_view_sideways_if_the_cursor_stays() {
        let display = numbered_rows(30);
        let mut window = Window::new(WindowId(0), BufferId(0));
        window.set_size(SIZE);
        window.selections = SelectionSet::new(5 * 3);
        window.scroll.left = 7;
        assert!(window.set_top(3, &display));
        assert_eq!(
            window.scroll,
            Scroll {
                start: 3 * 3,
                left: 7
            }
        );
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
        let mut window = Window::new(WindowId(0), BufferId(0));
        window.set_size(SIZE);
        let scroll = Scroll {
            start: 20 * 3,
            left: 3,
        };
        window.restore_positions(SelectionSet::new(5), scroll, &display);
        assert_eq!(window.scroll(), scroll);
        assert_eq!(window.selections.primary().cursor, 5, "out of view is kept");
        let past_the_end = Scroll {
            start: 999,
            left: 3,
        };
        window.restore_positions(SelectionSet::new(5), past_the_end, &display);
        assert_eq!(
            window.scroll(),
            Scroll {
                start: 30 * 3,
                left: 3
            }
        );
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
            let scroll = ViewTop { row: top, left };
            let next = follow(scroll, size, at(row, column), cells);
            prop_assert!((next.row..next.row + rows).contains(&row));
            prop_assert!((next.left..next.left + columns).contains(&column));
            let visible = (top..top + rows).contains(&row)
                && column >= left && column + cells <= left + columns;
            if visible {
                prop_assert_eq!(next, scroll);
            }
            // The fewest rows: the cursor ends at an edge if it moved.
            if next.row != top {
                prop_assert!(row == next.row || row == next.row + rows - 1);
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
