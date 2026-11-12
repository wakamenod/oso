//! The layout of a frame into regions, and the cells a window's cursor box and
//! the region between its marks and cursors take there (ADR 0026, ADR 0057,
//! ADR 0071).
//!
//! Must not depend on: `editor`, `command`, or any GPU or window-system type.
//! Sizes come in as plain numbers, positions through a display snapshot, and
//! nothing here draws.
//!
//! A frame is a grid of whole cells. From the top: the tiled windows, then the echo area.
//! [`Splits`] divides the tiled area among the windows (ADR 0112): each has its text area,
//! with a gutter for line numbers at its left (ADR 0097), its modeline below, and a divider
//! column at its right while a window is to its right. A frame too small for all of them
//! gives up the modeline first and the echo area next, so the text keeps a row
//! as long as there is one. The regions always add up to the frame, and never
//! overlap. Floating windows go above them and may cover them: [`float_regions`]
//! places one from the layout, which holds none (ADR 0072).
//!
//! [`region_cells`] says which cells of a window's text area its regions take,
//! so the renderer only fills them (ADR 0061, ADR 0071), [`match_cells`] those
//! isearch's matches take (ADR 0082), and [`other_cursor_cells`] those of the boxes of
//! the cursors but the primary (ADR 0091), all from the rows a frame shows (`cells.rs`,
//! ADR 0104).
//!
//! [`PixelGrid`] says where those cells are on the surface. The grid starts at
//! the top-left margin and holds whole cells only. The pixels left over at the
//! bottom and on the right are not part of any region.

use crate::window::{ViewSize, WindowId};

mod cells;
mod float;
mod splits;
pub(crate) use cells::match_cells_at;
pub(crate) use cells::match_cells_on_row;
pub use cells::{cursor_rect, match_cells, other_cursor_cells, region_cells};
pub(crate) use float::completion_regions;
pub use float::{FloatRegions, MIN_LIST_COLUMNS, float_regions};
pub(crate) use splits::Splits;
pub use splits::{BadTiling, Side, Tiling, TooSmall, Way};

/// The rows a frame reserves below the text area: the modeline and the echo
/// area.
const RESERVED_ROWS: usize = 2;

// Top-left margin, in physical pixels.
const MARGIN_X: f32 = 8.0;
const MARGIN_Y: f32 = 8.0;

/// A frame's size in whole cells. Either may be 0, for a surface smaller than
/// one cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FrameSize {
    pub rows: usize,
    pub columns: usize,
}

impl FrameSize {
    /// The frame whose text area is `view`, with no gutter.
    pub fn holding(view: ViewSize) -> Self {
        Self {
            rows: view.rows.saturating_add(RESERVED_ROWS),
            columns: view.columns,
        }
    }
}

/// A rectangle of cells on the frame's grid. It may be empty.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CellRect {
    /// The frame row of its top.
    pub row: usize,
    /// The frame column of its left edge.
    pub column: usize,
    pub rows: usize,
    pub columns: usize,
}

impl CellRect {
    /// The size scrolling works with: at least one row and column, even for an
    /// empty rectangle.
    pub fn view_size(self) -> ViewSize {
        ViewSize::new(self.rows, self.columns)
    }

    /// No cells, at the frame's top left.
    pub(crate) const EMPTY: Self = Self {
        row: 0,
        column: 0,
        rows: 0,
        columns: 0,
    };

    /// Whether the cell at `row` and `column` is in it.
    pub(crate) fn contains(self, row: usize, column: usize) -> bool {
        (self.row..self.row + self.rows).contains(&row)
            && (self.column..self.column + self.columns).contains(&column)
    }
}

/// The regions of one window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WindowRegions {
    /// The columns at the left of the text area, for line numbers (ADR 0097). None wide
    /// while they are off.
    pub gutter: CellRect,
    pub text: CellRect,
    /// Its bottom row, below its gutter and text.
    pub modeline: CellRect,
    /// Its right column, its modeline's row too, while a window is to its right (ADR 0112).
    pub divider: Option<CellRect>,
}

impl WindowRegions {
    /// The gutter and the text area together, where floats centre, so the gutter's width
    /// never moves one (ADR 0097).
    pub fn area(&self) -> CellRect {
        CellRect {
            column: self.gutter.column,
            columns: self.gutter.columns + self.text.columns,
            ..self.text
        }
    }

    /// None, for a window the frame has no room for.
    pub(crate) const EMPTY: Self = Self {
        gutter: CellRect::EMPTY,
        text: CellRect::EMPTY,
        modeline: CellRect::EMPTY,
        divider: None,
    };

    /// Whether the frame had room for it: a window it has none for gets empty regions.
    pub(crate) fn has_cells(&self) -> bool {
        let outer = self.outer();
        outer.rows > 0 && outer.columns > 0
    }

    /// All its cells: its gutter, text, modeline and divider.
    pub(crate) fn outer(&self) -> CellRect {
        CellRect {
            rows: self.text.rows + self.modeline.rows,
            columns: self.gutter.columns
                + self.text.columns
                + self.divider.map_or(0, |divider| divider.columns),
            ..self.area()
        }
    }
}

/// Which part of a window a cell is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Part {
    Text,
    Gutter,
    Modeline,
    Divider,
}

/// A cell of the tiled area: the window and the part of it it is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Hit {
    pub(crate) window: WindowId,
    pub(crate) part: Part,
}

/// A frame divided into regions.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FrameLayout {
    size: FrameSize,
    /// Each window's regions, in the tree's order.
    windows: Vec<(WindowId, WindowRegions)>,
    /// Each window's place in `windows`, by its id, for a lookup in O(log n) that a frame of
    /// many windows makes per window (ADR 0112).
    by_id: Vec<(WindowId, usize)>,
    pub echo: CellRect,
}

impl FrameLayout {
    /// The size of the frame it divides.
    pub fn size(&self) -> FrameSize {
        self.size
    }

    /// Each tiled window's regions, in the tree's order: left to right, top to bottom. A
    /// window the frame has no room for has none.
    pub fn windows(&self) -> &[(WindowId, WindowRegions)] {
        &self.windows
    }

    /// `window`'s regions, if it is a tiled window of this layout.
    pub fn regions(&self, window: WindowId) -> Option<&WindowRegions> {
        let at = self
            .by_id
            .binary_search_by_key(&window, |(id, _)| *id)
            .ok()?;
        self.windows
            .get(self.by_id[at].1)
            .map(|(_, regions)| regions)
    }

    /// Where floats go: the tiled area but the modelines on its bottom row, so they centre
    /// as they did on the one window's gutter and text, whatever the layout (ADR 0112).
    pub fn tiled(&self) -> CellRect {
        let rows = self.echo.row;
        let bottom = self
            .windows
            .iter()
            .filter(|(_, regions)| {
                let outer = regions.outer();
                outer.rows > 0 && outer.row + outer.rows == rows
            })
            .map(|(_, regions)| regions.modeline.rows)
            .max()
            .unwrap_or(0);
        CellRect {
            row: 0,
            column: 0,
            rows: rows - bottom,
            columns: self.size.columns,
        }
    }

    /// The window and part of it the cell at `row` and `column` is in, or `None` in the echo
    /// area and past the frame. The wheel picks its window through it (ADR 0112), and clicks
    /// and drags will select and resize through it.
    pub(crate) fn hit(&self, row: usize, column: usize) -> Option<Hit> {
        self.windows.iter().find_map(|(window, regions)| {
            let part = [
                (Part::Text, Some(regions.text)),
                (Part::Gutter, Some(regions.gutter)),
                (Part::Modeline, Some(regions.modeline)),
                (Part::Divider, regions.divider),
            ]
            .into_iter()
            .find_map(|(part, rect)| rect.filter(|r| r.contains(row, column)).map(|_| part))?;
            Some(Hit {
                window: *window,
                part,
            })
        })
    }

    /// The window next to `window` on `side` at `cell`: a row for the left and right, a
    /// column for up and down, where the window's primary cursor is.
    pub(crate) fn neighbour(&self, window: WindowId, side: Side, cell: usize) -> Option<WindowId> {
        let me = self.regions(window)?.outer();
        self.windows.iter().find_map(|(id, regions)| {
            let other = regions.outer();
            let touches = match side {
                Side::Left => other.column + other.columns == me.column,
                Side::Right => other.column == me.column + me.columns,
                Side::Up => other.row + other.rows == me.row,
                Side::Down => other.row == me.row + me.rows,
            };
            let across = match side {
                Side::Left | Side::Right => (other.row..other.row + other.rows).contains(&cell),
                Side::Up | Side::Down => {
                    (other.column..other.column + other.columns).contains(&cell)
                }
            };
            (*id != window && regions.has_cells() && touches && across).then_some(*id)
        })
    }
}

/// The gutter's columns in a frame of `size`: all of `gutter`, or none if the text area would
/// keep no column, so a number is never drawn cut.
pub fn fitting_gutter(size: FrameSize, gutter: usize) -> usize {
    if gutter < size.columns { gutter } else { 0 }
}

/// Divides a frame of `size` into regions: the echo area at the bottom, and the tiled area
/// above it among the windows of `splits`, each with a gutter of `gutter(window)` columns at
/// its left while its text area keeps a column, and none otherwise. A window the frame has
/// no room for gets no cells, but never `keep`, the selected one.
pub(crate) fn layout(
    size: FrameSize,
    splits: &Splits,
    keep: WindowId,
    gutter: impl Fn(WindowId) -> usize,
) -> FrameLayout {
    let mut frame = FrameLayout {
        size,
        windows: Vec::new(),
        by_id: Vec::new(),
        echo: CellRect::EMPTY,
    };
    layout_into(&mut frame, size, splits, keep, gutter);
    frame
}

/// [`layout`] into `frame`, reusing its room.
pub(crate) fn layout_into(
    frame: &mut FrameLayout,
    size: FrameSize,
    splits: &Splits,
    keep: WindowId,
    gutter: impl Fn(WindowId) -> usize,
) {
    let echo_rows = size.rows.saturating_sub(1).min(1);
    let tiled_rows = size.rows - echo_rows;
    let rows = |row, rows| CellRect {
        row,
        column: 0,
        rows,
        columns: size.columns,
    };
    frame.size = size;
    frame.echo = rows(tiled_rows, echo_rows);
    frame.windows.clear();
    splits.lay_out(rows(0, tiled_rows), keep, &gutter, &mut frame.windows);
    frame.by_id.clear();
    let places = frame.windows.iter().enumerate();
    frame.by_id.extend(places.map(|(at, (id, _))| (*id, at)));
    frame.by_id.sort_unstable();
}

/// A rectangle on the surface, in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PixelRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Where the frame's cells are on the surface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PixelGrid {
    origin: [f32; 2],
    cell: [f32; 2],
}

impl PixelGrid {
    /// The grid for cells of this size, in whole physical pixels, as
    /// `CellMetrics` gives them. A row count then survives the trip to pixels and
    /// back exactly.
    pub fn new(cell_width: f32, cell_height: f32) -> Self {
        Self {
            origin: [MARGIN_X, MARGIN_Y],
            cell: [cell_width, cell_height],
        }
    }

    /// The whole cells a surface of `width` x `height` physical pixels holds.
    pub fn frame_size(self, width: u32, height: u32) -> FrameSize {
        FrameSize {
            rows: whole_cells(height, self.origin[1], self.cell[1]),
            columns: whole_cells(width, self.origin[0], self.cell[0]),
        }
    }

    /// The top-left corner of the cell at `row` and `column` of the frame, in
    /// physical pixels. A cell scrolled out above or to the left of the grid has
    /// a negative row or column.
    pub fn cell(self, row: i64, column: i64) -> [f32; 2] {
        [
            self.origin[0] + column as f32 * self.cell[0],
            self.origin[1] + row as f32 * self.cell[1],
        ]
    }

    /// The row and column of the cell that holds the physical pixel at `x` and `y`, as the
    /// inverse of [`PixelGrid::cell`], for the pointer (ADR 0112). `None` in the margins to the
    /// left and above the grid, and for a place that is not finite. A pixel past the frame's
    /// last whole cell gives a cell past the frame, which no region holds.
    pub fn cell_at(self, x: f64, y: f64) -> Option<(usize, usize)> {
        let along = |pixel: f64, axis: usize| {
            let cells = (pixel - f64::from(self.origin[axis])) / f64::from(self.cell[axis]);
            // Saturates at usize::MAX, which no frame reaches.
            (cells.is_finite() && cells >= 0.0).then_some(cells as usize)
        };
        Some((along(y, 1)?, along(x, 0)?))
    }

    /// How wide a cell is, in physical pixels.
    pub(crate) fn cell_width(self) -> f32 {
        self.cell[0]
    }

    /// Where `cells` are, in physical pixels.
    pub fn rect(self, cells: CellRect) -> PixelRect {
        let [x, y] = self.cell(cells.row as i64, cells.column as i64);
        PixelRect {
            x,
            y,
            width: cells.columns as f32 * self.cell[0],
            height: cells.rows as f32 * self.cell[1],
        }
    }
}

/// How many cells of `cell` pixels fit in `pixels` after `margin`.
fn whole_cells(pixels: u32, margin: f32, cell: f32) -> usize {
    if cell > 0.0 {
        ((pixels as f32 - margin).max(0.0) / cell) as usize
    } else {
        0
    }
}

/// The layout of a frame of one window, numbered 0, for tests.
#[cfg(test)]
pub(crate) fn single(size: FrameSize, gutter: usize) -> FrameLayout {
    let one = WindowId::new(0);
    layout(size, &Splits::new(one), one, |_| gutter)
}

/// The regions of [`single`]'s window.
#[cfg(test)]
pub(crate) fn only(frame: &FrameLayout) -> WindowRegions {
    frame.windows()[0].1
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn frame(rows: usize, columns: usize) -> FrameLayout {
        single(FrameSize { rows, columns }, 0)
    }

    fn rect(row: usize, column: usize, rows: usize, columns: usize) -> CellRect {
        CellRect {
            row,
            column,
            rows,
            columns,
        }
    }

    #[test]
    fn a_float_takes_three_fifths_of_the_columns_from_the_top() {
        let layout = frame(30, 100);
        let regions = float_regions(&layout, 1);
        // The label, the gap, the answer and the padding below.
        assert_eq!(regions.outer, rect(0, 20, 4, 60));
        assert_eq!(regions.inner, rect(0, 22, 3, 56));
        assert!(regions.inner != regions.outer && regions.gap && regions.padded_below());
        assert_eq!(float_regions(&frame(30, 200), 1).outer, rect(0, 40, 4, 120));
        assert_eq!(float_regions(&layout, 3).outer, rect(0, 20, 6, 60));
    }

    #[test]
    fn a_float_takes_forty_columns_while_the_frame_keeps_two_at_each_edge() {
        assert_eq!(float_regions(&frame(30, 64), 1).outer, rect(0, 12, 4, 40));
        assert_eq!(float_regions(&frame(30, 30), 1).outer, rect(0, 2, 4, 26));
        assert_eq!(float_regions(&frame(30, 4), 1).outer.columns, 4);
    }

    #[test]
    fn a_completion_is_centered_with_its_rows_right_below_the_answer() {
        // 40 text rows: the answer and 18 rows, centered.
        let regions = completion_regions(&frame(42, 100), 18, false);
        assert_eq!(regions.outer, rect(10, 19, 19, 62));
        assert_eq!(regions.inner, rect(10, 20, 1, 60));
        assert_eq!(regions.candidates, rect(11, 20, 18, 60));
        assert!(!regions.gap && !regions.padded_below() && !regions.candidates_padded());
    }

    #[test]
    fn a_preview_takes_three_fifths_past_a_dividing_column_or_goes_when_narrow() {
        let regions = completion_regions(&frame(42, 100), 18, true);
        assert_eq!(regions.inner, rect(10, 20, 1, 60));
        assert_eq!(regions.candidates, rect(11, 20, 18, 24));
        assert_eq!(regions.preview, rect(11, 45, 18, 35));
        // At 62% of 80 columns the list would get 19, under its 20.
        let narrow = completion_regions(&frame(42, 80), 18, true);
        assert_eq!(narrow.candidates.columns, narrow.inner.columns);
        assert_eq!(narrow.preview.columns, 0);
        let none = completion_regions(&frame(42, 100), 18, false);
        assert_eq!(none.preview.columns, 0);
    }

    #[test]
    fn a_short_frame_gives_up_a_completions_rows() {
        let shape = |rows: usize| {
            let regions = completion_regions(&frame(rows + 2, 100), 18, false);
            (regions.outer.rows, regions.candidates.rows)
        };
        assert_eq!(shape(10), (10, 9));
        assert_eq!(shape(2), (2, 1));
        assert_eq!(shape(1), (1, 0));
    }

    #[test]
    fn a_narrow_float_keeps_less_padding_at_its_sides() {
        assert_eq!(float_regions(&frame(30, 16), 1).inner.columns, 8);
        assert_eq!(float_regions(&frame(30, 15), 1).inner.columns, 9);
        let regions = float_regions(&frame(30, 8), 1);
        assert_eq!(regions.inner.columns, regions.outer.columns);
    }

    #[test]
    fn a_short_frame_gives_up_the_padding_then_the_gap_then_the_label() {
        // Three text rows: no padding below.
        let regions = float_regions(&frame(5, 100), 1);
        assert_eq!(regions.outer, rect(0, 20, 3, 60));
        assert!(regions.gap && !regions.padded_below());
        // Two: no gap either.
        let regions = float_regions(&frame(4, 100), 1);
        assert_eq!((regions.inner.rows, regions.gap), (2, false));
        // Two, for a label of three rows: one of them and the answer.
        assert_eq!(float_regions(&frame(4, 100), 3).inner.rows, 2);
        // One: the label and the answer share it.
        let regions = float_regions(&frame(3, 100), 3);
        assert_eq!(regions.outer, rect(0, 20, 1, 60));
        assert_eq!(regions.inner, rect(0, 22, 1, 56));
        // A frame of one row has the float over it.
        assert_eq!(float_regions(&frame(1, 100), 1).outer, rect(0, 20, 1, 60));
    }

    #[test]
    fn the_modeline_and_the_echo_area_take_the_bottom_rows() {
        let layout = frame(24, 80);
        assert_eq!(only(&layout).text, rect(0, 0, 22, 80));
        assert_eq!(only(&layout).modeline, rect(22, 0, 1, 80));
        assert_eq!(layout.echo, rect(23, 0, 1, 80));
    }

    #[test]
    fn a_small_frame_gives_up_the_modeline_then_the_echo_area() {
        let rows = |layout: FrameLayout| {
            (
                only(&layout).text.rows,
                only(&layout).modeline.rows,
                layout.echo.rows,
            )
        };
        assert_eq!(rows(frame(3, 80)), (1, 1, 1));
        assert_eq!(rows(frame(2, 80)), (1, 0, 1));
        assert_eq!(rows(frame(1, 80)), (1, 0, 0));
        assert_eq!(rows(frame(0, 80)), (0, 0, 0));
    }

    #[test]
    fn a_frame_with_no_cells_still_gives_a_size_to_scroll_with() {
        let layout = frame(0, 0);
        assert_eq!(only(&layout).text.view_size(), ViewSize::new(1, 1));
    }

    #[test]
    fn a_very_large_frame_does_not_overflow() {
        let layout = frame(usize::MAX, usize::MAX);
        assert_eq!(only(&layout).text.rows, usize::MAX - 2);
        assert_eq!(only(&layout).modeline.row, usize::MAX - 2);
        assert_eq!(layout.echo.row, usize::MAX - 1);
        assert_eq!(layout.echo.columns, usize::MAX);
    }

    #[test]
    fn a_frame_holding_a_view_gives_it_back_as_the_text_area() {
        let view = ViewSize::default();
        let layout = single(FrameSize::holding(view), 0);
        assert_eq!(only(&layout).text.view_size(), view);
        assert_eq!(only(&layout).modeline.rows, 1);
        assert_eq!(layout.echo.rows, 1);
    }

    #[test]
    fn leftover_pixels_belong_to_no_cell() {
        let grid = PixelGrid::new(10.0, 20.0);
        // 8 + 3 * 20 = 68 and 8 + 5 * 10 = 58.
        for (width, height) in [(58, 68), (67, 87)] {
            let size = FrameSize {
                rows: 3,
                columns: 5,
            };
            assert_eq!(grid.frame_size(width, height), size);
        }
        assert_eq!(grid.frame_size(57, 67).rows, 2);
        assert_eq!(grid.frame_size(57, 67).columns, 4);
    }

    #[test]
    fn a_surface_smaller_than_the_margin_holds_no_cells() {
        let grid = PixelGrid::new(10.0, 20.0);
        let none = FrameSize {
            rows: 0,
            columns: 0,
        };
        assert_eq!(grid.frame_size(0, 0), none);
        assert_eq!(grid.frame_size(1, 1), none);
    }

    #[test]
    fn a_huge_surface_holds_a_huge_frame() {
        let frame = PixelGrid::new(10.0, 20.0).frame_size(u32::MAX, u32::MAX);
        assert!(frame.rows > 200_000_000 && frame.columns > 400_000_000);
    }

    #[test]
    fn cells_with_no_size_hold_nothing() {
        for size in [0.0, -1.0, f32::NAN] {
            let frame = PixelGrid::new(size, size).frame_size(800, 600);
            assert_eq!((frame.rows, frame.columns), (0, 0));
        }
    }

    #[test]
    fn a_rectangle_of_cells_starts_after_the_margin() {
        let grid = PixelGrid::new(10.0, 20.0);
        assert_eq!(
            grid.rect(rect(2, 3, 4, 5)),
            PixelRect {
                x: 38.0,
                y: 48.0,
                width: 50.0,
                height: 80.0,
            }
        );
    }

    #[test]
    fn a_cell_scrolled_out_of_the_grid_has_a_place_too() {
        let grid = PixelGrid::new(10.0, 20.0);
        assert_eq!(grid.cell(0, 0), [8.0, 8.0]);
        assert_eq!(grid.cell(-1, -2), [-12.0, -12.0]);
    }

    #[test]
    fn a_pixel_is_in_the_cell_whose_square_holds_it() {
        let grid = PixelGrid::new(10.0, 20.0);
        assert_eq!(
            grid.cell_at(8.0, 8.0),
            Some((0, 0)),
            "the first cell's corner"
        );
        assert_eq!(grid.cell_at(17.9, 27.9), Some((0, 0)));
        assert_eq!(grid.cell_at(18.0, 28.0), Some((1, 1)));
        assert_eq!(grid.cell_at(43.0, 75.0), Some((3, 3)));
        assert_eq!(
            grid.cell_at(1e9, 1e9),
            Some((49_999_999, 99_999_999)),
            "past the frame, a cell no region holds"
        );
    }

    #[test]
    fn a_pixel_in_the_margins_or_nowhere_is_in_no_cell() {
        let grid = PixelGrid::new(10.0, 20.0);
        for (x, y) in [
            (7.9, 20.0),
            (20.0, 7.9),
            (0.0, 0.0),
            (-30.0, 40.0),
            (f64::NAN, 40.0),
            (40.0, f64::INFINITY),
            (f64::NEG_INFINITY, 40.0),
        ] {
            assert_eq!(grid.cell_at(x, y), None, "at {x}, {y}");
        }
        assert_eq!(PixelGrid::new(0.0, 0.0).cell_at(20.0, 20.0), None);
    }

    proptest! {
        /// Every pixel of a cell is in it, from its corner to its far edge (ADR 0112).
        #[test]
        fn prop_a_cell_holds_its_own_pixels(
            row in 0usize..10_000, column in 0usize..10_000,
            width in 1u32..40, height in 1u32..80, dx in 0.0f64..1.0, dy in 0.0f64..1.0,
        ) {
            let grid = PixelGrid::new(width as f32, height as f32);
            let [x, y] = grid.cell(row as i64, column as i64);
            let (x, y) = (f64::from(x) + dx * f64::from(width - 1).max(0.5), f64::from(y) + dy * f64::from(height - 1).max(0.5));
            prop_assert_eq!(grid.cell_at(x, y), Some((row, column)));
        }
    }

    proptest! {
        /// The regions fill the frame from the top, in order and with no gap.
        #[test]
        fn prop_regions_add_up_to_the_frame(rows in 0usize..100, columns in 0usize..100) {
            let layout = frame(rows, columns);
            let (text, modeline, echo) = (only(&layout).text, only(&layout).modeline, layout.echo);
            prop_assert_eq!(text.row, 0);
            prop_assert_eq!(modeline.row, text.rows);
            prop_assert_eq!(echo.row, text.rows + modeline.rows);
            prop_assert_eq!(echo.row + echo.rows, rows);
            prop_assert_eq!(text.rows.min(1), rows.min(1), "the text keeps a row");
            for region in [text, modeline, echo] {
                prop_assert_eq!((region.column, region.columns), (0, columns));
            }
        }

        /// The gutter and the text area share the window's columns, the text keeps one, and a
        /// float is where it would be with no gutter (ADR 0097).
        #[test]
        fn prop_a_gutter_takes_from_the_text_and_moves_no_float(
            rows in 0usize..60, columns in 0usize..200, gutter in 0usize..20,
            content in 0usize..10, listed in 0usize..30,
        ) {
            let size = FrameSize { rows, columns };
            let (with, without) = (single(size, gutter), single(size, 0));
            let window = only(&with);
            prop_assert_eq!(window.gutter.columns + window.text.columns, columns);
            prop_assert_eq!(window.text.column, window.gutter.columns);
            prop_assert!(window.text.columns > 0 || columns == 0);
            prop_assert_eq!(window.area(), only(&without).text);
            prop_assert_eq!(with.size(), size);
            prop_assert_eq!(float_regions(&with, content), float_regions(&without, content));
            prop_assert_eq!(
                completion_regions(&with, listed, true),
                completion_regions(&without, listed, true)
            );
        }

        /// A float stays inside the text area, its inner cells inside it, and leaves the
        /// modeline and the echo area alone.
        #[test]
        fn prop_a_float_stays_in_the_text_area(
            rows in 0usize..60, columns in 0usize..300, content in 0usize..30,
            listed in 0usize..30,
        ) {
            let layout = frame(rows, columns);
            let text = only(&layout).text;
            let regions = if listed == 0 {
                float_regions(&layout, content)
            } else {
                completion_regions(&layout, listed, false)
            };
            let FloatRegions { outer, inner, candidates, .. } = regions;
            // The candidates, inside the float and below what it shows, never over it.
            if candidates.rows > 0 {
                prop_assert!(candidates.row >= inner.row + inner.rows);
                prop_assert!(candidates.row + candidates.rows <= outer.row + outer.rows);
                prop_assert_eq!((candidates.column, candidates.columns), (inner.column, inner.columns));
                prop_assert!(candidates.rows <= listed);
            }
            prop_assert!(outer.row >= text.row && outer.row + outer.rows <= text.row + text.rows);
            prop_assert!(outer.column >= text.column);
            prop_assert!(outer.column + outer.columns <= text.column + text.columns);
            prop_assert!(inner.row >= outer.row && inner.column >= outer.column);
            prop_assert!(inner.row + inner.rows <= outer.row + outer.rows);
            prop_assert!(inner.column + inner.columns <= outer.column + outer.columns);
        }
    }
}
