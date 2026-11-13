//! Tests of a terminal's grid in a pane: its cells' colours and styles, wide and grapheme
//! cells, the cursor's shapes, a preedit, clipping and padding, the shaping cache, and what
//! must not reorder or hide (ADR 0120).
//!
//! Must not depend on: `capture`, `paint` or a font backend; a fake text system shapes.

use oso_core::display_map::Preedit;
use oso_core::frame::CellRect;
use oso_platform::text::GlyphKey;

use crate::pane::{
    Face, GridCell, GridColor, GridCursor, GridCursorShape, GridRow, GridStyle, GridView, Pane,
    PaneContent,
};
use crate::scene::tests::{FakeAtlas, LIMITS, build, pane, rect, theme};
use crate::scene::{Limits, Scene};
use crate::shape::Rect;

mod cost;
mod cursor;

/// A cell of `ch` in the default colours.
pub(super) fn plain(ch: char) -> GridCell {
    GridCell {
        ch,
        fg: GridColor::Default,
        bg: GridColor::Default,
        underline_color: GridColor::Default,
        style: GridStyle::default(),
    }
}

/// One cell a character of `text`, in the default colours.
pub(super) fn cells(text: &str) -> Vec<GridCell> {
    text.chars().map(plain).collect()
}

/// A row of `cells` with no graphemes.
pub(super) fn row(generation: u64, cells: &[GridCell]) -> GridRow<'_> {
    GridRow {
        generation,
        cells,
        text: "",
        graphemes: &[],
    }
}

/// Grid 7's `rows`, with `cursor` and no preedit.
pub(super) fn grid<'a>(rows: &'a [GridRow<'a>], cursor: Option<GridCursor>) -> PaneContent<'a> {
    PaneContent::Grid(GridView {
        id: 7,
        rows,
        cursor,
        preedit: None,
    })
}

/// The pane of `content` in `region`, with no background of its own.
pub(super) fn grid_pane<'a>(region: CellRect, content: PaneContent<'a>) -> Pane<'a> {
    pane(0, region, 0, content)
}

pub(super) fn cursor(column: u16, shape: GridCursorShape, hollow: bool) -> Option<GridCursor> {
    Some(GridCursor {
        row: 0,
        column,
        shape,
        hollow,
    })
}

/// The characters whose glyphs were looked up: the fake text system's glyph is the character.
pub(super) fn looked_up(atlas: &FakeAtlas) -> String {
    let char_of = |key: &GlyphKey| char::from_u32(u32::from(key.glyph.0)).unwrap_or('?');
    atlas.looked_up.iter().map(char_of).collect()
}

/// Each quad's left edge and top, as `(x, y)`, and its colour. Cells are 10 x 20 px from a
/// margin of 8 px, and the fake glyph sits 1 px in and 4 px down.
pub(super) fn quads(scene: &Scene) -> Vec<((f32, f32), [f32; 4])> {
    let quads = scene.quads().iter();
    quads
        .map(|q| ((q.position()[0], q.position()[1]), q.color()))
        .collect()
}

/// Each rectangle's position, size and colour.
pub(super) fn rects(scene: &Scene) -> Vec<([f32; 2], [f32; 2], [f32; 4])> {
    let rects = scene.rects().iter();
    rects
        .map(|r: &Rect| (r.position, r.size, r.color))
        .collect()
}

/// The frame of `panes` in a new scene, its atlas, and what was shaped.
pub(super) fn draw(panes: &[Pane<'_>]) -> (Scene, FakeAtlas, Vec<String>) {
    let mut scene = Scene::new(LIMITS, theme());
    let mut atlas = FakeAtlas::default();
    let system = build(&mut scene, panes, &mut atlas);
    (scene, atlas, system.shaped)
}

pub(super) fn fg(face: Face) -> [f32; 4] {
    theme().foreground(face)
}

pub(super) fn bg(face: Face) -> [f32; 4] {
    theme().background(face)
}

#[test]
fn a_cells_colours_are_the_defaults_a_face_or_its_own_and_inverse_swaps_them() {
    let cells = [
        plain('a'),
        GridCell {
            fg: GridColor::Face(Face::TerminalColor1),
            bg: GridColor::Face(Face::TerminalColor4),
            ..plain('b')
        },
        GridCell {
            fg: GridColor::Rgb([255, 0, 0]),
            bg: GridColor::Rgb([0, 0, 255]),
            ..plain('c')
        },
        GridCell {
            style: GridStyle::INVERSE,
            ..plain('d')
        },
    ];
    let rows = [row(1, &cells)];
    let (scene, _, _) = draw(&[grid_pane(rect(0, 0, 1, 10), grid(&rows, None))]);
    let colours: Vec<_> = quads(&scene).into_iter().map(|(_, c)| c).collect();
    let red = [1.0, 0.0, 0.0, 1.0];
    assert_eq!(
        colours,
        [
            fg(Face::Text),
            fg(Face::TerminalColor1),
            red,
            bg(Face::Text)
        ]
    );
    // The pane's text background, then each cell's that is not the default, a face's
    // foreground for a face.
    let blue = [0.0, 0.0, 1.0, 1.0];
    assert_eq!(
        rects(&scene),
        [
            ([8.0, 8.0], [100.0, 20.0], bg(Face::Text)),
            ([18.0, 8.0], [10.0, 20.0], fg(Face::TerminalColor4)),
            ([28.0, 8.0], [10.0, 20.0], blue),
            ([38.0, 8.0], [10.0, 20.0], fg(Face::Text)),
        ]
    );
}

#[test]
fn a_pane_with_the_text_background_is_not_filled_twice_and_a_run_is_one_rectangle() {
    let red = GridCell {
        bg: GridColor::Rgb([255, 0, 0]),
        ..plain('a')
    };
    let cells = [red, red, red, plain('b'), red];
    let rows = [row(1, &cells)];
    let mut panes = [grid_pane(rect(0, 0, 1, 10), grid(&rows, None))];
    panes[0].background = Some(Face::Text);
    let (scene, _, _) = draw(&panes);
    let sizes: Vec<_> = rects(&scene).iter().map(|r| (r.0[0], r.1[0])).collect();
    assert_eq!(sizes, [(8.0, 100.0), (8.0, 30.0), (48.0, 10.0)]);
}

#[test]
fn a_wide_cell_spans_its_spacer_and_the_spacer_draws_only_its_background() {
    let cells = [
        GridCell {
            style: GridStyle::WIDE,
            ..plain('日')
        },
        GridCell {
            bg: GridColor::Rgb([255, 255, 255]),
            style: GridStyle::WIDE_SPACER,
            ..plain(' ')
        },
        plain('x'),
    ];
    let rows = [row(1, &cells)];
    let (scene, atlas, shaped) = draw(&[grid_pane(rect(0, 0, 1, 10), grid(&rows, None))]);
    assert_eq!(shaped, ["日x"], "one piece, the spacer in no glyph's place");
    assert_eq!(looked_up(&atlas), "日x");
    // Centred in its two cells, 5 px in, and `x` in the third.
    let lefts: Vec<_> = quads(&scene).iter().map(|q| q.0.0).collect();
    assert_eq!(lefts, [14.0, 29.0]);
    let filled = rects(&scene)[1];
    assert_eq!(
        (filled.0, filled.1),
        ([18.0, 8.0], [10.0, 20.0]),
        "the spacer"
    );
}

#[test]
fn a_grapheme_cell_takes_its_text_from_the_row() {
    let cells = [
        plain('a'),
        GridCell {
            style: GridStyle::GRAPHEME,
            ..plain('e')
        },
        plain('b'),
    ];
    let graphemes = [(1, 0..3)];
    let rows = [GridRow {
        generation: 1,
        cells: &cells,
        text: "e\u{301}",
        graphemes: &graphemes,
    }];
    let (scene, atlas, shaped) = draw(&[grid_pane(rect(0, 0, 1, 10), grid(&rows, None))]);
    assert_eq!(shaped, ["ae\u{301}b"]);
    assert_eq!(looked_up(&atlas), "ae\u{301}b");
    let lefts: Vec<_> = quads(&scene).iter().map(|q| q.0.0).collect();
    // The letter and its mark stay in the second cell, from x 18 to 28.
    assert_eq!(lefts[0], 9.0);
    assert!(
        lefts[1..3].iter().all(|x| (18.0..28.0).contains(x)),
        "{lefts:?}"
    );
    assert_eq!(lefts[3], 29.0);
}

#[test]
fn rows_and_cells_past_the_pane_are_clipped_and_not_read() {
    let cells = cells("abcdefgh");
    let rows = [row(1, &cells), row(1, &cells), row(1, &cells)];
    let (scene, atlas, shaped) = draw(&[grid_pane(rect(1, 2, 2, 5), grid(&rows, None))]);
    assert_eq!(shaped, ["abcde", "abcde"]);
    assert_eq!(looked_up(&atlas), "abcdeabcde");
    for ((x, y), _) in quads(&scene) {
        assert!(
            (28.0..78.0).contains(&x) && (28.0..68.0).contains(&y),
            "{x} {y}"
        );
    }
}

#[test]
fn what_the_rows_leave_of_the_pane_shows_the_text_background() {
    let cells = cells("abc");
    let rows = [row(1, &cells), row(1, &cells)];
    let (scene, _, _) = draw(&[grid_pane(rect(0, 0, 4, 12), grid(&rows, None))]);
    assert_eq!(rects(&scene), [([8.0, 8.0], [120.0, 80.0], bg(Face::Text))]);
    assert_eq!(quads(&scene).len(), 6);
}

#[test]
fn a_row_is_reused_while_its_generation_holds_and_read_again_when_it_changes() {
    let (one, two) = (cells("abc"), cells("xyz"));
    let mut scene = Scene::new(LIMITS, theme());
    let mut frame = |rows: &[GridRow<'_>], id: u64| {
        let content = PaneContent::Grid(GridView {
            id,
            rows,
            cursor: None,
            preedit: None,
        });
        let mut atlas = FakeAtlas::default();
        let panes = [grid_pane(rect(0, 0, 2, 10), content)];
        let system = build(&mut scene, &panes, &mut atlas);
        (system.shaped, looked_up(&atlas))
    };
    assert_eq!(frame(&[row(1, &one), row(1, &one)], 7).0, ["abc", "abc"]);
    // Other cells at the same generation are not read: the generation is what says.
    let (shaped, drawn) = frame(&[row(1, &one), row(1, &two)], 7);
    assert!(shaped.is_empty());
    assert_eq!(drawn, "abcabc");
    let (shaped, drawn) = frame(&[row(1, &one), row(2, &two)], 7);
    assert_eq!(shaped, ["xyz"]);
    assert_eq!(drawn, "abcxyz");
    // A new generation of the same text reads it, and shapes nothing.
    assert!(frame(&[row(1, &one), row(3, &two)], 7).0.is_empty());
    // Another grid shapes all again.
    assert_eq!(frame(&[row(1, &one), row(3, &two)], 8).0, ["abc", "xyz"]);
}

#[test]
fn a_switch_of_theme_recolours_cells_and_shapes_nothing() {
    let cells = [GridCell {
        fg: GridColor::Face(Face::TerminalColor2),
        bg: GridColor::Face(Face::TerminalColor0),
        ..plain('a')
    }];
    let rows = [row(1, &cells)];
    let panes = [grid_pane(rect(0, 0, 1, 10), grid(&rows, None))];
    let mut scene = Scene::new(LIMITS, theme());
    build(&mut scene, &panes, &mut FakeAtlas::default());
    let light = crate::theme::themes()[1];
    scene.set_theme(light);
    let system = build(&mut scene, &panes, &mut FakeAtlas::default());
    assert!(system.shaped.is_empty());
    assert_eq!(quads(&scene)[0].1, light.foreground(Face::TerminalColor2));
    assert_eq!(rects(&scene)[1].2, light.foreground(Face::TerminalColor0));
}

#[test]
fn a_piece_is_a_run_of_one_look_with_its_blanks_but_those_at_its_end() {
    let red = |ch| GridCell {
        fg: GridColor::Rgb([255, 0, 0]),
        ..plain(ch)
    };
    let cells = [
        plain('a'),
        plain('b'),
        red('c'),
        red('d'),
        plain(' '),
        plain('e'),
        plain('\0'),
        plain('f'),
        plain(' '),
        plain('\0'),
    ];
    let rows = [row(1, &cells)];
    let (_, atlas, shaped) = draw(&[grid_pane(rect(0, 0, 1, 10), grid(&rows, None))]);
    // The space ends `cd`, as it is not red, the one never written stays in `e f`, and those
    // at the end are not shaped.
    assert_eq!(shaped, ["ab", "cd", "e f"]);
    assert_eq!(looked_up(&atlas), "abcdef", "no glyph for a blank");
}

#[test]
fn a_bidi_control_is_boxed_and_never_shaped_and_reorders_nothing() {
    let cells = cells("a\u{202e}bc");
    let rows = [row(1, &cells)];
    let (scene, atlas, shaped) = draw(&[grid_pane(rect(0, 0, 1, 10), grid(&rows, None))]);
    assert_eq!(shaped, ["a", "bc"]);
    assert_eq!(looked_up(&atlas), "abc");
    let lefts: Vec<_> = quads(&scene).iter().map(|q| q.0.0).collect();
    assert_eq!(lefts, [9.0, 29.0, 39.0], "each in its own cell, in order");
    assert_eq!(
        rects(&scene)[1],
        ([18.0, 8.0], [10.0, 20.0], bg(Face::Boxed))
    );
}

#[test]
fn a_right_to_left_cell_is_shaped_alone_and_keeps_its_column() {
    let cells = cells("ab\u{5d0}\u{5d1}cd");
    let rows = [row(1, &cells)];
    let (scene, _, shaped) = draw(&[grid_pane(rect(0, 0, 1, 10), grid(&rows, None))]);
    assert_eq!(shaped, ["ab", "\u{5d0}", "\u{5d1}", "cd"]);
    let lefts: Vec<_> = quads(&scene).iter().map(|q| q.0.0).collect();
    assert_eq!(lefts, [9.0, 19.0, 29.0, 39.0, 49.0, 59.0]);
}

#[test]
fn a_letter_an_invisible_character_extends_is_boxed_in_the_boxs_colours() {
    let cells = [
        plain('x'),
        GridCell {
            fg: GridColor::Rgb([0, 255, 0]),
            style: GridStyle::GRAPHEME,
            ..plain('a')
        },
        GridCell {
            style: GridStyle::GRAPHEME,
            ..plain('b')
        },
    ];
    let graphemes = [(1, 0..4), (2, 4..8)];
    let rows = [GridRow {
        generation: 1,
        cells: &cells,
        text: "a\u{200b}\u{200b}b",
        graphemes: &graphemes,
    }];
    let (scene, atlas, shaped) = draw(&[grid_pane(rect(0, 0, 1, 10), grid(&rows, None))]);
    assert_eq!(shaped, ["x", "a"]);
    assert_eq!(
        looked_up(&atlas),
        "xa",
        "an invisible first character draws nothing"
    );
    assert_eq!(quads(&scene)[1].1, fg(Face::Boxed));
    // Two boxes side by side, the first a pixel short.
    assert_eq!(
        rects(&scene)[1..],
        [
            ([18.0, 8.0], [9.0, 20.0], bg(Face::Boxed)),
            ([28.0, 8.0], [10.0, 20.0], bg(Face::Boxed)),
        ]
    );
}

#[test]
fn a_row_past_its_budget_cuts_a_cell_of_several_characters_to_its_first() {
    // Four characters a column: three columns hold twelve.
    let cells: Vec<_> = (0..3)
        .map(|_| GridCell {
            style: GridStyle::GRAPHEME,
            ..plain('e')
        })
        .collect();
    let whole = "e\u{301}\u{302}\u{303}\u{304}\u{305}\u{306}";
    let text = whole.repeat(3);
    let one = whole.len() as u32;
    let graphemes: Vec<_> = (0..3u16)
        .map(|i| (i, u32::from(i) * one..(u32::from(i) + 1) * one))
        .collect();
    let rows = [GridRow {
        generation: 1,
        cells: &cells,
        text: &text,
        graphemes: &graphemes,
    }];
    let (scene, _, shaped) = draw(&[grid_pane(rect(0, 0, 1, 3), grid(&rows, None))]);
    assert_eq!(shaped, [format!("{whole}{whole}"), "e".to_owned()]);
    let last = *rects(&scene).last().unwrap();
    assert_eq!(last, ([28.0, 8.0], [10.0, 20.0], bg(Face::Boxed)));
}

#[test]
fn lines_dim_and_hidden_cells_are_drawn_as_their_styles_say() {
    let styled = |ch, style| GridCell { style, ..plain(ch) };
    let cells = [
        styled('a', GridStyle::UNDERLINE),
        GridCell {
            underline_color: GridColor::Rgb([255, 0, 0]),
            ..styled('b', GridStyle::DOUBLE_UNDERLINE)
        },
        styled('c', GridStyle::STRIKE),
        styled('d', GridStyle::HIDDEN),
        styled('e', GridStyle::DIM),
        styled('f', GridStyle::BOLD.with(GridStyle::ITALIC)),
    ];
    let rows = [row(1, &cells)];
    let (scene, atlas, shaped) = draw(&[grid_pane(rect(0, 0, 1, 10), grid(&rows, None))]);
    assert_eq!(shaped.len(), 6, "each its own piece: {shaped:?}");
    assert_eq!(looked_up(&atlas), "abcef", "a hidden cell draws no glyph");
    let red = [1.0, 0.0, 0.0, 1.0];
    let text = fg(Face::Text);
    assert_eq!(
        rects(&scene)[1..],
        [
            ([8.0, 27.0], [10.0, 1.0], text),
            ([18.0, 27.0], [10.0, 1.0], red),
            ([18.0, 25.0], [10.0, 1.0], red),
            ([28.0, 18.0], [10.0, 1.0], text),
        ]
    );
    let back = bg(Face::Text);
    let dim: [f32; 4] = std::array::from_fn(|i| (text[i] + back[i]) / 2.0);
    let colours: Vec<_> = quads(&scene).into_iter().map(|(_, c)| c).collect();
    assert_eq!(
        colours,
        [text, text, text, dim, text],
        "bold and italic drawn regular"
    );
}

#[test]
fn an_empty_grid_and_an_empty_pane_draw_only_the_background() {
    let (scene, _, shaped) = draw(&[grid_pane(rect(0, 0, 2, 4), grid(&[], None))]);
    assert_eq!(rects(&scene).len(), 1);
    assert!(shaped.is_empty() && scene.quads().is_empty());
    let cells = cells("ab");
    let rows = [row(1, &cells)];
    let content = grid(&rows, cursor(0, GridCursorShape::Block, false));
    let (scene, _, shaped) = draw(&[grid_pane(rect(0, 0, 0, 0), content)]);
    assert!(scene.rects().is_empty() && scene.quads().is_empty() && shaped.is_empty());
}

/// The cell at column `i` of a grid row for [`prop_a_grid_draws_only_in_its_region`]: wide
/// cells, spacers, a boxed bidi control and Hebrew, each with a style that draws lines.
fn mixed(i: usize) -> GridCell {
    match i % 4 {
        0 => GridCell {
            style: GridStyle::WIDE.with(GridStyle::UNDERLINE),
            ..plain('日')
        },
        1 => GridCell {
            style: GridStyle::WIDE_SPACER,
            bg: GridColor::Rgb([9, 9, 9]),
            ..plain(' ')
        },
        2 => GridCell {
            style: GridStyle::STRIKE.with(GridStyle::INVERSE),
            ..plain('\u{202e}')
        },
        _ => GridCell {
            style: GridStyle::DOUBLE_UNDERLINE,
            ..plain('\u{5d0}')
        },
    }
}

proptest::proptest! {
    /// Whatever the region, the grid's size, the cursor and the preedit, everything a grid
    /// draws stays in its region, but for the band above the frame's top row.
    #[test]
    fn prop_a_grid_draws_only_in_its_region(
        (row, column, rows, columns) in (0usize..6, 0usize..12, 0usize..6, 0usize..12),
        (grid_rows, grid_columns, shift) in (0usize..8, 0usize..16, 0usize..4),
        (cursor_row, cursor_column) in (0u16..9, 0u16..17),
        (shape, hollow, composing) in (0usize..3, proptest::bool::ANY, proptest::bool::ANY),
    ) {
        let line: Vec<_> = (shift..shift + grid_columns).map(mixed).collect();
        let grid_rows: Vec<_> = (0..grid_rows).map(|_| super::tests::row(1, &line)).collect();
        let shapes = [GridCursorShape::Block, GridCursorShape::Bar, GridCursorShape::Underline];
        let preedit = Preedit::new("\u{3042}\u{202e}".to_owned(), Some(0..0));
        let content = PaneContent::Grid(GridView {
            id: 1,
            rows: &grid_rows,
            cursor: Some(GridCursor {
                row: cursor_row,
                column: cursor_column,
                shape: shapes[shape],
                hollow,
            }),
            preedit: composing.then_some(&preedit),
        });
        let panes = [grid_pane(rect(row, column, rows, columns), content)];
        let limits = Limits { rects: 1000, quads: 1000 };
        let mut scene = Scene::new(limits, theme());
        build(&mut scene, &panes, &mut FakeAtlas::default());
        let (x0, y0) = (8.0 + column as f32 * 10.0, 8.0 + row as f32 * 20.0);
        let (x1, y1) = (x0 + columns as f32 * 10.0, y0 + rows as f32 * 20.0);
        let band = if row == 0 { 4.0 } else { 0.0 };
        for rect in scene.rects() {
            let ([x, y], [w, h]) = (rect.position, rect.size);
            proptest::prop_assert!(x >= x0 && y >= y0 && x + w <= x1 && y + h <= y1);
            proptest::prop_assert!(w > 0.0 && h > 0.0);
        }
        for quad in scene.quads() {
            let ([x, y], [w, h]) = (quad.position(), quad.size());
            proptest::prop_assert!(x >= x0 && y >= y0 - band && x + w <= x1 && y + h <= y1);
        }
    }
}
