//! Tests of the bar on a float's selected row (ADR 0076, ADR 0088): across the panel, but
//! beside a preview only as far as the column that divides the list from it, so that it marks
//! no line of the preview.
//!
//! Must not depend on: the window system, the renderer or the GPU; these build regions in
//! memory.

use oso_core::frame::{CellRect, FloatRegions};

use super::bar;

fn rect(row: usize, column: usize, rows: usize, columns: usize) -> CellRect {
    CellRect {
        row,
        column,
        rows,
        columns,
    }
}

/// A float at column 10, 60 wide, whose candidates' rows are 4 to 13, with a preview right of
/// the divider at column 33 if `preview`.
fn regions(preview: bool) -> FloatRegions {
    let (list, preview) = if preview {
        (rect(4, 12, 10, 21), rect(4, 34, 10, 34))
    } else {
        (rect(4, 12, 10, 56), rect(4, 12, 0, 0))
    };
    FloatRegions {
        outer: rect(1, 10, 14, 60),
        inner: rect(2, 12, 12, 56),
        gap: true,
        candidates: list,
        preview,
    }
}

#[test]
fn a_selected_candidate_s_bar_goes_across_the_panel_with_no_preview() {
    assert_eq!(bar(&regions(false), 7, true), rect(7, 10, 1, 60));
}

#[test]
fn beside_a_preview_the_bar_ends_at_the_column_that_divides_them() {
    let regions = regions(true);
    let bar = bar(&regions, 7, true);
    assert_eq!(bar, rect(7, 10, 1, 23));
    // The divider is the preview's column less one: the bar takes no cell of the preview.
    assert_eq!(bar.column + bar.columns, regions.preview.column - 1);
}

#[test]
fn the_answer_s_row_keeps_its_bar_across_the_panel_above_a_preview() {
    assert_eq!(bar(&regions(true), 2, false), rect(2, 10, 1, 60));
}
