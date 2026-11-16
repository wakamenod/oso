//! Tests of where a float at point goes (ADR 0138, steps 9 and 10): below the anchor's row, or
//! above it when more room is there, never over it, moved left to fit, and cut to the room.
//!
//! Must not depend on: `editor`, `command`, or any GPU or window-system type.

use super::*;

const AREA: CellRect = CellRect {
    row: 0,
    column: 4,
    rows: 20,
    columns: 40,
};

#[test]
fn it_goes_below_the_anchor_with_its_text_at_the_anchor_s_column() {
    let float = at_point(AREA, (3, 10), (5, 12)).unwrap();
    assert_eq!(
        float.outer,
        CellRect {
            row: 4,
            column: 9,
            rows: 5,
            columns: 14
        }
    );
    assert_eq!((float.inner.column, float.inner.columns), (10, 12));
    assert_eq!(float.candidates.rows, 0);
}

#[test]
fn it_goes_above_when_more_rows_fit_there_and_never_over_the_anchor_s_row() {
    let float = at_point(AREA, (17, 10), (10, 12)).unwrap();
    assert_eq!((float.outer.row, float.outer.rows), (7, 10));
    let tight = at_point(AREA, (15, 10), (10, 12)).unwrap();
    assert_eq!(
        (tight.outer.row, tight.outer.rows),
        (5, 10),
        "above, as only 4 fit below"
    );
    let cut = at_point(CellRect { rows: 6, ..AREA }, (2, 10), (10, 12)).unwrap();
    assert_eq!(
        (cut.outer.row, cut.outer.rows),
        (3, 3),
        "cut to the room below"
    );
}

#[test]
fn it_moves_left_to_fit_and_is_cut_to_the_area_s_width() {
    let float = at_point(AREA, (0, 40), (5, 12)).unwrap();
    assert_eq!(
        float.outer.column + float.outer.columns,
        AREA.column + AREA.columns
    );
    let wide = at_point(AREA, (0, 6), (5, 100)).unwrap();
    assert_eq!((wide.outer.column, wide.outer.columns), (4, 40));
}

#[test]
fn nothing_goes_outside_the_area_or_where_nothing_fits() {
    assert!(at_point(AREA, (25, 10), (5, 12)).is_none());
    assert!(at_point(AREA, (3, 2), (5, 12)).is_none());
    let one_row = CellRect { rows: 1, ..AREA };
    assert!(at_point(one_row, (0, 10), (5, 12)).is_none());
}
