//! The layout of a frame into regions, and the cells a window's cursor box and
//! the region between its marks and cursors take there (ADR 0026, ADR 0057,
//! ADR 0071).
//!
//! Must not depend on: `editor`, `command`, or any GPU or window-system type.
//! Sizes come in as plain numbers, positions through a display snapshot, and
//! nothing here draws.
//!
//! A frame is a grid of whole cells. From the top: the one tiled window's text
//! area, its modeline, then the echo area. A frame too small for all of them
//! gives up the modeline first and the echo area next, so the text keeps a row
//! as long as there is one. The regions always add up to the frame, and never
//! overlap. Floating windows go above them and may cover them: [`float_regions`]
//! places one from the layout, which holds none (ADR 0072).
//!
//! [`region_cells`] says which cells of a window's text area its regions take,
//! so the renderer only fills them (ADR 0061, ADR 0071), and [`match_cells`] those
//! isearch's matches take (ADR 0082).
//!
//! [`PixelGrid`] says where those cells are on the surface. The grid starts at
//! the top-left margin and holds whole cells only. The pixels left over at the
//! bottom and on the right are not part of any region.

use std::ops::Range;

use crate::display_map::{DisplayPoint, DisplaySnapshot};
use crate::selection::{Selection, SelectionSet};
use crate::sum_tree::Bias;
use crate::window::{Scroll, ViewSize};

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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FrameLayout {
    /// The one tiled window, until splits arrive (M3).
    pub window: WindowRegions,
    pub echo: CellRect,
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
    }
}

/// The cells a float takes over the frame (ADR 0072, ADR 0075).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FloatRegions {
    /// All of it: the cells its panel covers, padding included.
    pub outer: CellRect,
    /// What it shows, inside the padding: the label's rows, the gap and the answer's row.
    /// The same as `outer` where no padding fits.
    pub inner: CellRect,
    /// Whether a row of padding goes between the label and the answer.
    pub gap: bool,
}

impl FloatRegions {
    /// Whether a row of padding goes below what it shows.
    pub fn padded_below(&self) -> bool {
        self.outer.row + self.outer.rows > self.inner.row + self.inner.rows
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
/// text area in `frame` and centered, as the minibuffer does (ADR 0026, ADR 0075). It takes
/// 60% of the columns, at least 40, and never comes closer than 2 to an edge. Below the
/// label come a row of padding, the answer's row and a row of padding, with 2 columns of
/// padding on each side. A small frame gives up the padding below first, then the gap, then
/// the label's rows; with one row of text left, the label and the answer share it. It is
/// never taller than the text area, so the modeline and the echo area stay in view.
pub fn float_regions(frame: &FrameLayout, rows: usize) -> FloatRegions {
    let text = frame.window.text;
    let room = text.columns.saturating_sub(2 * FLOAT_EDGE);
    let columns = if room == 0 {
        text.columns
    } else {
        (text.columns * FLOAT_TENTHS / 10)
            .max(FLOAT_MIN_COLUMNS)
            .min(room)
    };
    let side = if columns >= 12 {
        FLOAT_SIDE
    } else {
        usize::from(columns >= 5)
    };
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
    FloatRegions { outer, inner, gap }
}

/// The cells of a cursor at `point`, whose cluster takes `cells` cells, in a
/// window with this `text` area and `scroll`. A cluster that an edge of the area
/// cuts keeps the part in view, and `None` means none of it is.
pub fn cursor_rect(
    text: CellRect,
    scroll: Scroll,
    point: DisplayPoint,
    cells: usize,
) -> Option<CellRect> {
    let row = point.row.checked_sub(scroll.top)?;
    let start = point.column.max(scroll.left);
    let end = point
        .column
        .saturating_add(cells.max(1))
        .min(scroll.left.saturating_add(text.columns));
    if row >= text.rows || start >= end {
        return None;
    }
    Some(CellRect {
        row: text.row + row,
        column: text.column + (start - scroll.left),
        rows: 1,
        columns: end - start,
    })
}

/// The cells of the regions of `selections`, in a window with this `text` area and
/// `scroll`, into `out`: one rectangle for each row of a region in view, and none
/// while the regions are hidden. A row whose line break is in a region is filled to
/// the area's right edge, as Emacs's region face with `:extend` (ADR 0071).
/// `preedit` is the cursor's point and the cells of a preedit there, which pushes
/// what follows it on that row, so a region from that point starts after it.
///
/// Each row looks only at the regions in its columns in view, found by a binary
/// search on the selections, so a frame costs two conversions per row and two for
/// each region a row shows, however long the rows and however many regions are
/// out of view.
pub fn region_cells(
    text: CellRect,
    scroll: Scroll,
    display: &DisplaySnapshot,
    selections: &SelectionSet,
    preedit: Option<(DisplayPoint, usize)>,
    out: &mut Vec<CellRect>,
) {
    out.clear();
    if !selections.mark_active().is_active() {
        return;
    }
    // While the regions show, none overlap, so their ends are in order too.
    let place = Place {
        text,
        scroll,
        display,
        preedit,
    };
    range_cells(place, selections.as_slice(), Selection::range, out);
}

/// The cells of `ranges`, which are in order and do not overlap, in a window as
/// [`region_cells`] takes it, into `out`, after what it holds: as a region's, with a row
/// whose line break is in one filled to the right edge. For isearch's matches (ADR 0082).
pub fn match_cells(
    text: CellRect,
    scroll: Scroll,
    display: &DisplaySnapshot,
    ranges: &[Range<usize>],
    preedit: Option<(DisplayPoint, usize)>,
    out: &mut Vec<CellRect>,
) {
    let place = Place {
        text,
        scroll,
        display,
        preedit,
    };
    range_cells(place, ranges, Range::clone, out);
}

/// Where cells of ranges go: a window's text area, its view, and a preedit in it.
#[derive(Clone, Copy)]
struct Place<'a> {
    text: CellRect,
    scroll: Scroll,
    display: &'a DisplaySnapshot,
    preedit: Option<(DisplayPoint, usize)>,
}

/// The cells of `items`' ranges, in order and none overlapping, into `out`, as
/// [`region_cells`] says.
fn range_cells<T>(
    place: Place<'_>,
    all: &[T],
    range_of: impl Fn(&T) -> Range<usize>,
    out: &mut Vec<CellRect>,
) {
    let Place {
        text,
        scroll,
        display,
        preedit,
    } = place;
    let rows = scroll.top
        ..scroll
            .top
            .saturating_add(text.rows)
            .min(display.row_count());
    let columns = scroll.left..scroll.left.saturating_add(text.columns);
    let mut first = 0;
    for row in rows.clone() {
        let pushed = preedit.filter(|(at, _)| at.row == row);
        // A preedit can bring into view what is left of the area in the buffer.
        let left = columns
            .start
            .saturating_sub(pushed.map_or(0, |(_, cells)| cells));
        let offset =
            |column| display.display_point_to_offset(DisplayPoint::new(row, column), Bias::Left);
        let (shown_start, shown_end) = (offset(left), offset(columns.end));
        first += all[first..].partition_point(|item| range_of(item).end <= shown_start);
        for item in &all[first..] {
            let range = range_of(item);
            if range.start > shown_end {
                break;
            }
            if range.is_empty() {
                continue;
            }
            let start = display.offset_to_display_point(range.start);
            let end = display.offset_to_display_point(range.end);
            let mut from = if row == start.row { start.column } else { 0 };
            let mut to = if row == end.row {
                end.column
            } else {
                usize::MAX
            };
            if let Some((at, cells)) = pushed {
                if from >= at.column {
                    from = from.saturating_add(cells);
                }
                if to > at.column {
                    to = to.saturating_add(cells);
                }
            }
            let (from, to) = (from.max(columns.start), to.min(columns.end));
            if from < to {
                out.push(CellRect {
                    row: text.row + (row - rows.start),
                    column: text.column + (from - columns.start),
                    rows: 1,
                    columns: to - from,
                });
            }
        }
    }
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
        assert_eq!(layout.window.text, rect(0, 0, 22, 80));
        assert_eq!(layout.window.modeline, rect(22, 0, 1, 80));
        assert_eq!(layout.echo, rect(23, 0, 1, 80));
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
    fn a_cell_scrolled_out_of_the_grid_has_a_place_too() {
        let grid = PixelGrid::new(10.0, 20.0);
        assert_eq!(grid.cell(0, 0), [8.0, 8.0]);
        assert_eq!(grid.cell(-1, -2), [-12.0, -12.0]);
    }

    #[test]
    fn the_cursor_cell_moves_with_the_scroll_position() {
        let text = rect(0, 0, 10, 20);
        let scroll = Scroll { top: 5, left: 2 };
        assert_eq!(
            cursor_rect(text, scroll, DisplayPoint::new(7, 3), 1),
            Some(rect(2, 1, 1, 1))
        );
        assert_eq!(
            cursor_rect(text, scroll, DisplayPoint::new(5, 2), 2),
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
            assert_eq!(cursor_rect(text, scroll, point, 1), None, "{point:?}");
        }
        let empty = rect(0, 0, 0, 20);
        let point = DisplayPoint::new(0, 0);
        assert_eq!(cursor_rect(empty, Scroll::default(), point, 1), None);
    }

    #[test]
    fn a_cursor_rectangle_stops_at_the_right_edge() {
        let text = rect(0, 0, 10, 20);
        let point = DisplayPoint::new(0, 19);
        let cells = cursor_rect(text, Scroll::default(), point, 2);
        assert_eq!(cells, Some(rect(0, 19, 1, 1)));
    }

    #[test]
    fn a_cursor_rectangle_stops_at_the_left_edge() {
        let text = rect(0, 0, 10, 20);
        let scroll = Scroll { top: 0, left: 5 };
        let at = |column, cells| cursor_rect(text, scroll, DisplayPoint::new(0, column), cells);
        assert_eq!(
            at(4, 2),
            Some(rect(0, 0, 1, 1)),
            "the right half of a wide one"
        );
        assert_eq!(at(3, 6), Some(rect(0, 0, 1, 4)), "the end of a long label");
        assert_eq!(at(3, 2), None, "it ends just before the edge");
    }

    #[test]
    fn a_cursor_rectangle_cut_on_both_sides_fills_the_area() {
        let text = rect(0, 0, 10, 4);
        let scroll = Scroll { top: 0, left: 5 };
        let cells = cursor_rect(text, scroll, DisplayPoint::new(0, 3), 64);
        assert_eq!(cells, Some(rect(0, 0, 1, 4)));
    }

    #[test]
    fn a_cursor_rectangle_keeps_to_an_area_away_from_the_origin() {
        let text = rect(3, 7, 10, 20);
        let scroll = Scroll { top: 5, left: 2 };
        let cells = cursor_rect(text, scroll, DisplayPoint::new(6, 1), 2);
        assert_eq!(cells, Some(rect(4, 7, 1, 1)));
    }

    #[test]
    fn huge_positions_do_not_overflow() {
        let text = rect(0, 0, 10, 20);
        let far = DisplayPoint::new(0, usize::MAX);
        let scroll = Scroll { top: 0, left: 5 };
        assert_eq!(cursor_rect(text, scroll, far, usize::MAX), None);
        let scroll = Scroll {
            top: 0,
            left: usize::MAX,
        };
        let cells = cursor_rect(text, scroll, far, 1);
        assert_eq!(cells, None, "the column past the last one is not in view");
    }

    mod regions {
        use super::*;
        use crate::display_map::{DisplayMap, TabWidth};
        use crate::rope::Rope;
        use crate::selection::{MarkActive, Selection};

        // Rows: "abcdef", "xy", "", "hello", and the empty last one.
        const TEXT: &str = "abcdef\nxy\n\nhello\n";

        fn cells(
            columns: usize,
            scroll: Scroll,
            parts: &[(usize, usize)],
            region: MarkActive,
            preedit: Option<(DisplayPoint, usize)>,
        ) -> Vec<(usize, usize, usize)> {
            let display = DisplayMap::new(&Rope::from(TEXT), TabWidth::default()).snapshot();
            let selections = parts.iter().map(|&(cursor, mark)| Selection {
                mark: Some(mark),
                ..Selection::at(cursor)
            });
            let mut set = SelectionSet::from_parts(selections.collect(), 0).unwrap();
            set.set_mark_active(region);
            let text = rect(0, 0, 4, columns);
            let mut out = vec![rect(9, 9, 9, 9)];
            region_cells(text, scroll, &display, &set, preedit, &mut out);
            out.iter()
                .inspect(|cell| assert_eq!(cell.rows, 1))
                .map(|cell| (cell.row, cell.column, cell.columns))
                .collect()
        }

        const AT_TOP: Scroll = Scroll { top: 0, left: 0 };

        #[test]
        fn a_region_fills_its_rows_and_to_the_edge_where_its_line_break_is() {
            // From `c` to `l` of `hello`, the mark first.
            let found = cells(5, AT_TOP, &[(13, 2)], MarkActive::Active, None);
            assert_eq!(found, [(0, 2, 3), (1, 0, 5), (2, 0, 5), (3, 0, 2)]);
            let backward = cells(5, AT_TOP, &[(2, 13)], MarkActive::Shifted, None);
            assert_eq!(backward, found, "either way round");
        }

        #[test]
        fn hidden_or_empty_regions_take_no_cells() {
            assert_eq!(cells(5, AT_TOP, &[(13, 2)], MarkActive::Inactive, None), []);
            assert_eq!(cells(5, AT_TOP, &[(3, 3)], MarkActive::Active, None), []);
            // Up to the start of `xy`: only the line break of the row before.
            assert_eq!(
                cells(9, AT_TOP, &[(0, 7)], MarkActive::Active, None),
                [(0, 0, 9)]
            );
        }

        #[test]
        fn only_the_part_in_view_is_taken_in_the_areas_cells() {
            let left = Scroll { top: 0, left: 1 };
            let found = cells(5, left, &[(13, 2)], MarkActive::Active, None);
            assert_eq!(found, [(0, 1, 4), (1, 0, 5), (2, 0, 5), (3, 0, 1)]);
            let down = Scroll { top: 2, left: 0 };
            let found = cells(5, down, &[(13, 2)], MarkActive::Active, None);
            assert_eq!(found, [(0, 0, 5), (1, 0, 2)]);
            let below = Scroll { top: 3, left: 0 };
            assert_eq!(
                cells(5, below, &[(1, 0)], MarkActive::Active, None),
                [],
                "above it"
            );
        }

        #[test]
        fn each_region_in_view_takes_its_own_cells() {
            let found = cells(
                9,
                AT_TOP,
                &[(1, 0), (5, 3), (9, 8)],
                MarkActive::Active,
                None,
            );
            assert_eq!(found, [(0, 0, 1), (0, 3, 2), (1, 1, 1)]);
        }

        #[test]
        fn a_preedit_pushes_a_region_after_it_and_not_one_that_ends_there() {
            let at = Some((DisplayPoint::new(0, 2), 4));
            // From the preedit's place to `e`.
            assert_eq!(
                cells(9, AT_TOP, &[(2, 4)], MarkActive::Active, at),
                [(0, 6, 2)]
            );
            // From `a` to the preedit's place.
            assert_eq!(
                cells(9, AT_TOP, &[(2, 0)], MarkActive::Active, at),
                [(0, 0, 2)]
            );
            let other_row = Some((DisplayPoint::new(1, 0), 4));
            assert_eq!(
                cells(9, AT_TOP, &[(2, 4)], MarkActive::Active, other_row),
                [(0, 2, 2)]
            );
        }
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

        /// A float stays inside the text area, its inner cells inside it, and leaves the
        /// modeline and the echo area alone.
        #[test]
        fn prop_a_float_stays_in_the_text_area(
            rows in 0usize..60, columns in 0usize..300, content in 0usize..30,
        ) {
            let layout = frame(rows, columns);
            let text = layout.window.text;
            let FloatRegions { outer, inner, .. } = float_regions(&layout, content);
            prop_assert!(outer.row >= text.row && outer.row + outer.rows <= text.row + text.rows);
            prop_assert!(outer.column >= text.column);
            prop_assert!(outer.column + outer.columns <= text.column + text.columns);
            prop_assert!(inner.row >= outer.row && inner.column >= outer.column);
            prop_assert!(inner.row + inner.rows <= outer.row + outer.rows);
            prop_assert!(inner.column + inner.columns <= outer.column + outer.columns);
        }

        /// The cursor's cells are inside the text area, and are exactly the part of
        /// its cluster in view.
        #[test]
        fn prop_a_cursor_rectangle_is_the_part_in_view(
            rows in 0usize..100, columns in 0usize..100,
            top in 0usize..100, left in 0usize..100,
            row in 0usize..200, column in 0usize..200, cells in 1usize..70,
        ) {
            let text = frame(rows, columns).window.text;
            let scroll = Scroll { top, left };
            let found = cursor_rect(text, scroll, DisplayPoint::new(row, column), cells);
            let visible = (left..left + text.columns)
                .filter(|c| (column..column + cells).contains(c))
                .count();
            let in_rows = (top..top + text.rows).contains(&row);
            prop_assert_eq!(found.is_some(), in_rows && visible > 0);
            if let Some(cell) = found {
                prop_assert_eq!((cell.rows, cell.columns), (1, visible));
                prop_assert_eq!(cell.row, row - top);
                prop_assert!(cell.column + cell.columns <= text.columns);
            }
        }
    }
}
