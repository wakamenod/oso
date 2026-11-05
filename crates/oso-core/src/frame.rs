//! The layout of a frame into regions (ADR 0026, ADR 0057).
//!
//! Must not depend on: `editor`, `command`, or any GPU or window-system type.
//! Sizes come in as plain numbers, and nothing here draws.
//!
//! A frame is a grid of whole cells. From the top: the selected window's text
//! area, its modeline, then the echo area. A frame too small for all of them
//! gives up the modeline first and the echo area next, so the text keeps a row
//! as long as there is one. The regions always add up to the frame, and never
//! overlap.
//!
//! [`PixelGrid`] says where those cells are on the surface. The grid starts at
//! the top-left margin and holds whole cells only. The pixels left over at the
//! bottom and on the right are not part of any region.

use crate::display_map::DisplayPoint;
use crate::window::{Scroll, ViewSize};

/// The rows a frame reserves below the text area: the modeline and the echo
/// area.
const RESERVED_ROWS: usize = 2;

// Top-left margin, in physical pixels (will become the line-number gutter
// offset in the future).
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
    /// The frame whose text area is `view`.
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
}

/// The regions of one window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WindowRegions {
    pub text: CellRect,
    pub modeline: CellRect,
}

/// A frame divided into regions.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FrameLayout {
    /// The selected window, the only one until splits arrive (M3).
    pub window: WindowRegions,
    pub echo: CellRect,
    /// Floating windows, back to front, above all the other regions and free to
    /// cover them (ADR 0008). Each feature decides where its own goes, so this is
    /// empty until M2-16.
    pub floating: Vec<CellRect>,
}

/// Divides a frame of `size` into regions.
pub fn layout(size: FrameSize) -> FrameLayout {
    let echo_rows = size.rows.saturating_sub(1).min(1);
    let window_rows = size.rows - echo_rows;
    let modeline_rows = window_rows.saturating_sub(1).min(1);
    let text_rows = window_rows - modeline_rows;
    let rows = |row, rows| CellRect {
        row,
        column: 0,
        rows,
        columns: size.columns,
    };
    FrameLayout {
        window: WindowRegions {
            text: rows(0, text_rows),
            modeline: rows(text_rows, modeline_rows),
        },
        echo: rows(window_rows, echo_rows),
        floating: Vec::new(),
    }
}

/// The cells of a cursor at `point`, whose cluster takes `cells` cells, in a
/// window with this `text` area and `scroll`. `None` if it is out of view. A
/// cluster that the right edge cuts ends there.
pub fn cursor_cell(
    text: CellRect,
    scroll: Scroll,
    point: DisplayPoint,
    cells: usize,
) -> Option<CellRect> {
    let row = point.row.checked_sub(scroll.top)?;
    let column = point.column.checked_sub(scroll.left)?;
    if row >= text.rows || column >= text.columns {
        return None;
    }
    Some(CellRect {
        row: text.row + row,
        column: text.column + column,
        rows: 1,
        columns: cells.clamp(1, text.columns - column),
    })
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
    /// The grid for cells of this size, in physical pixels.
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

    /// Where `cells` are, in physical pixels.
    pub fn rect(self, cells: CellRect) -> PixelRect {
        PixelRect {
            x: self.origin[0] + cells.column as f32 * self.cell[0],
            y: self.origin[1] + cells.row as f32 * self.cell[1],
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

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn frame(rows: usize, columns: usize) -> FrameLayout {
        layout(FrameSize { rows, columns })
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
    fn the_modeline_and_the_echo_area_take_the_bottom_rows() {
        let layout = frame(24, 80);
        assert_eq!(layout.window.text, rect(0, 0, 22, 80));
        assert_eq!(layout.window.modeline, rect(22, 0, 1, 80));
        assert_eq!(layout.echo, rect(23, 0, 1, 80));
        assert!(layout.floating.is_empty());
    }

    #[test]
    fn a_small_frame_gives_up_the_modeline_then_the_echo_area() {
        let rows = |layout: FrameLayout| {
            (
                layout.window.text.rows,
                layout.window.modeline.rows,
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
        assert_eq!(layout.window.text.view_size(), ViewSize::new(1, 1));
    }

    #[test]
    fn a_very_large_frame_does_not_overflow() {
        let layout = frame(usize::MAX, usize::MAX);
        assert_eq!(layout.window.text.rows, usize::MAX - 2);
        assert_eq!(layout.window.modeline.row, usize::MAX - 2);
        assert_eq!(layout.echo.row, usize::MAX - 1);
        assert_eq!(layout.echo.columns, usize::MAX);
    }

    #[test]
    fn a_frame_holding_a_view_gives_it_back_as_the_text_area() {
        let view = ViewSize::default();
        let layout = layout(FrameSize::holding(view));
        assert_eq!(layout.window.text.view_size(), view);
        assert_eq!(layout.window.modeline.rows, 1);
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
    fn the_cursor_cell_moves_with_the_scroll_position() {
        let text = rect(0, 0, 10, 20);
        let scroll = Scroll { top: 5, left: 2 };
        assert_eq!(
            cursor_cell(text, scroll, DisplayPoint::new(7, 3), 1),
            Some(rect(2, 1, 1, 1))
        );
        assert_eq!(
            cursor_cell(text, scroll, DisplayPoint::new(5, 2), 2),
            Some(rect(0, 0, 1, 2)),
            "a wide cluster takes both cells",
        );
    }

    #[test]
    fn a_cursor_out_of_view_has_no_cell() {
        let text = rect(0, 0, 10, 20);
        let scroll = Scroll { top: 5, left: 2 };
        for (row, column) in [(4, 3), (15, 3), (7, 1), (7, 22)] {
            let point = DisplayPoint::new(row, column);
            assert_eq!(cursor_cell(text, scroll, point, 1), None, "{point:?}");
        }
        let empty = rect(0, 0, 0, 20);
        let point = DisplayPoint::new(0, 0);
        assert_eq!(cursor_cell(empty, Scroll::default(), point, 1), None);
    }

    #[test]
    fn a_cursor_cell_stops_at_the_right_edge() {
        let text = rect(0, 0, 10, 20);
        let point = DisplayPoint::new(0, 19);
        let cell = cursor_cell(text, Scroll::default(), point, 2);
        assert_eq!(cell, Some(rect(0, 19, 1, 1)));
    }

    proptest! {
        /// The regions fill the frame from the top, in order and with no gap.
        #[test]
        fn prop_regions_add_up_to_the_frame(rows in 0usize..100, columns in 0usize..100) {
            let layout = frame(rows, columns);
            let (text, modeline, echo) = (layout.window.text, layout.window.modeline, layout.echo);
            prop_assert_eq!(text.row, 0);
            prop_assert_eq!(modeline.row, text.rows);
            prop_assert_eq!(echo.row, text.rows + modeline.rows);
            prop_assert_eq!(echo.row + echo.rows, rows);
            prop_assert_eq!(text.rows.min(1), rows.min(1), "the text keeps a row");
            for region in [text, modeline, echo] {
                prop_assert_eq!((region.column, region.columns), (0, columns));
            }
        }

        /// The cursor's cells are always inside the text area.
        #[test]
        fn prop_a_cursor_cell_stays_in_the_text_area(
            rows in 0usize..100, columns in 0usize..100,
            top in 0usize..100, left in 0usize..100,
            row in 0usize..200, column in 0usize..200, cells in 1usize..70,
        ) {
            let text = frame(rows, columns).window.text;
            let scroll = Scroll { top, left };
            if let Some(cell) = cursor_cell(text, scroll, DisplayPoint::new(row, column), cells) {
                prop_assert!(cell.rows == 1 && cell.columns >= 1);
                prop_assert!(cell.row + cell.rows <= text.rows);
                prop_assert!(cell.column + cell.columns <= text.columns);
            }
        }
    }
}
