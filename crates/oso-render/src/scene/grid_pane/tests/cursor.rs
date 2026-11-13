//! Tests of a terminal's cursor in a pane, in each of its shapes, and of a preedit over the
//! cells from the cursor's (ADR 0120, ADR 0069).
//!
//! Must not depend on: `capture`, `paint` or a font backend; a fake text system shapes.

use oso_core::display_map::Preedit;

use super::{bg, cells, cursor, draw, fg, grid, grid_pane, looked_up, plain, quads, rects, row};
use crate::pane::{Face, GridCell, GridCursorShape, GridStyle, GridView, PaneContent};
use crate::scene::tests::rect;

#[test]
fn a_block_cursor_is_the_cursor_box_with_its_glyph_in_the_boxs_colour() {
    let cells = cells("ab");
    let rows = [row(1, &cells)];
    let content = grid(&rows, cursor(1, GridCursorShape::Block, false));
    let (scene, _, _) = draw(&[grid_pane(rect(0, 0, 1, 10), content)]);
    let last = *rects(&scene).last().unwrap();
    assert_eq!(last, ([18.0, 8.0], [10.0, 20.0], bg(Face::Cursor)));
    let colours: Vec<_> = quads(&scene).into_iter().map(|(_, c)| c).collect();
    assert_eq!(colours, [fg(Face::Text), fg(Face::Cursor)]);
}

#[test]
fn a_block_cursor_on_a_wide_cell_covers_both_its_cells() {
    let cells = [
        GridCell {
            style: GridStyle::WIDE,
            ..plain('日')
        },
        GridCell {
            style: GridStyle::WIDE_SPACER,
            ..plain(' ')
        },
    ];
    let rows = [row(1, &cells)];
    let content = grid(&rows, cursor(0, GridCursorShape::Block, false));
    let (scene, _, _) = draw(&[grid_pane(rect(0, 0, 1, 10), content)]);
    let last = *rects(&scene).last().unwrap();
    assert_eq!((last.0, last.1), ([8.0, 8.0], [20.0, 20.0]));
}

#[test]
fn a_bar_is_at_the_cells_left_and_an_underline_at_its_bottom_over_glyphs_as_they_are() {
    let cells = cells("ab");
    let rows = [row(1, &cells)];
    // A line is a point, the cell's height over 16, in whole pixels: 1 px. These are two.
    for (shape, want) in [
        (GridCursorShape::Bar, ([18.0, 8.0], [2.0, 20.0])),
        (GridCursorShape::Underline, ([18.0, 26.0], [10.0, 2.0])),
    ] {
        let content = grid(&rows, cursor(1, shape, false));
        let (scene, _, _) = draw(&[grid_pane(rect(0, 0, 1, 10), content)]);
        let last = *rects(&scene).last().unwrap();
        assert_eq!(last, (want.0, want.1, bg(Face::Cursor)), "{shape:?}");
        let colours: Vec<_> = quads(&scene).into_iter().map(|(_, c)| c).collect();
        assert_eq!(colours, [fg(Face::Text); 2], "{shape:?}");
    }
}

#[test]
fn a_hollow_cursor_is_its_cells_outline_whatever_its_shape_and_none_draws_nothing() {
    let cells = cells("ab");
    let rows = [row(1, &cells)];
    for shape in [
        GridCursorShape::Block,
        GridCursorShape::Bar,
        GridCursorShape::Underline,
    ] {
        let content = grid(&rows, cursor(1, shape, true));
        let (scene, _, _) = draw(&[grid_pane(rect(0, 0, 1, 10), content)]);
        let sides = &rects(&scene)[1..];
        assert_eq!(sides.len(), 4, "{shape:?}");
        assert!(sides.iter().all(|side| side.2 == bg(Face::Cursor)));
        let colours: Vec<_> = quads(&scene).into_iter().map(|(_, c)| c).collect();
        assert_eq!(colours, [fg(Face::Text); 2], "glyphs as they are");
    }
    let (scene, _, _) = draw(&[grid_pane(rect(0, 0, 1, 10), grid(&rows, None))]);
    assert_eq!(rects(&scene).len(), 1, "the text's background only");
}

#[test]
fn a_cursor_out_of_the_pane_draws_nothing() {
    let cells = cells("ab");
    let rows = [row(1, &cells)];
    let content = grid(&rows, cursor(5, GridCursorShape::Block, false));
    let (scene, _, _) = draw(&[grid_pane(rect(0, 0, 1, 4), content)]);
    assert_eq!(rects(&scene).len(), 1);
}

#[test]
fn a_preedit_covers_the_cells_from_the_cursors_and_is_underlined() {
    let cells = cells("abcdef");
    let rows = [row(1, &cells)];
    let preedit = Preedit::new("xy".to_owned(), None);
    let content = PaneContent::Grid(GridView {
        id: 7,
        rows: &rows,
        cursor: cursor(1, GridCursorShape::Block, false),
        preedit: Some(&preedit),
        preedit_at: (0, 1),
    });
    let (scene, atlas, shaped) = draw(&[grid_pane(rect(0, 0, 1, 10), content)]);
    // `b` and `c` are under it, and the cells after it are where they were.
    assert_eq!(looked_up(&atlas), "adefxy");
    assert_eq!(shaped, ["abcdef", "xy"]);
    let drawn: Vec<_> = quads(&scene)
        .into_iter()
        .map(|((x, _), c)| (x, c))
        .collect();
    let text = fg(Face::Text);
    assert_eq!(
        drawn,
        [
            (9.0, text),
            (39.0, fg(Face::Cursor)),
            (49.0, text),
            (59.0, text),
            (19.0, fg(Face::Preedit)),
            (29.0, fg(Face::Preedit)),
        ],
        "the box after the preedit, on `d`"
    );
    let all = rects(&scene);
    assert_eq!(
        all[1],
        ([18.0, 8.0], [20.0, 20.0], bg(Face::Text)),
        "its cover"
    );
    assert_eq!(all[2], ([18.0, 27.0], [20.0, 1.0], bg(Face::Preedit)));
    assert_eq!(all[3], ([38.0, 8.0], [10.0, 20.0], bg(Face::Cursor)));
}

#[test]
fn a_preedits_caret_takes_the_cursor_box_and_its_segment_two_lines() {
    let cells = cells("abcdef");
    let rows = [row(1, &cells)];
    let caret = Preedit::new("xyz".to_owned(), Some(1..1));
    let segment = Preedit::new("xyz".to_owned(), Some(1..3));
    let content = |preedit| {
        PaneContent::Grid(GridView {
            id: 7,
            rows: &rows,
            cursor: cursor(1, GridCursorShape::Block, false),
            preedit: Some(preedit),
            preedit_at: (0, 1),
        })
    };
    let (scene, _, _) = draw(&[grid_pane(rect(0, 0, 1, 10), content(&caret))]);
    let last = *rects(&scene).last().unwrap();
    assert_eq!(last, ([28.0, 8.0], [10.0, 20.0], bg(Face::Cursor)));
    let y = quads(&scene).into_iter().find(|q| q.0.0 == 29.0).unwrap();
    assert_eq!(y.1, fg(Face::Cursor), "`y`, under the box");
    let (scene, _, _) = draw(&[grid_pane(rect(0, 0, 1, 10), content(&segment))]);
    let lines: Vec<_> = rects(&scene)
        .into_iter()
        // The cursor box, after the preedit, is the same colour.
        .filter(|r| r.2 == bg(Face::Preedit) && r.1[1] < 20.0)
        .map(|r| (r.0, r.1))
        .collect();
    assert_eq!(
        lines,
        [([18.0, 27.0], [30.0, 1.0]), ([28.0, 26.0], [20.0, 2.0])]
    );
}

#[test]
fn a_preedit_is_drawn_at_its_cell_while_the_program_hides_the_cursor() {
    let cells = cells("abcdef");
    let rows = [row(1, &cells)];
    let preedit = Preedit::new("xy".to_owned(), None);
    let with = |preedit| {
        PaneContent::Grid(GridView {
            id: 7,
            rows: &rows,
            cursor: None,
            preedit,
            preedit_at: (0, 1),
        })
    };
    let (bare, _, _) = draw(&[grid_pane(rect(0, 0, 1, 10), with(None))]);
    let (composed, _, shaped) = draw(&[grid_pane(rect(0, 0, 1, 10), with(Some(&preedit)))]);
    // Its text is shaped, and its cover and underlines are drawn, with no cursor box.
    assert!(shaped.iter().any(|text| text == "xy"), "{shaped:?}");
    assert!(rects(&composed).len() > rects(&bare).len());
}
