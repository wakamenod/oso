//! Tests of the lines along a pane's edges (ADR 0170): where they are, what is over them, and that
//! no text of the pane is drawn where one is.
//!
//! Must not depend on: `capture`, `paint` or a font backend; a fake text system shapes.

use oso_core::buffer::Buffer;
use oso_core::display_map::{DisplayPoint, Preedit};
use oso_core::editor::Editor;
use oso_core::frame::CellRect;
use oso_core::selection::SelectionSet;
use oso_core::syntax::{Highlight, Highlights};
use oso_core::window::{Screen, ViewSize, ViewTop};

use crate::pane::{
    Bands, CursorStyle, Face, FlashFill, Pane, PaneContent, Rules, Span, TextMarks, TextView,
};
use crate::scene::Scene;
use crate::scene::tests::{FakeAtlas, LIMITS, build, id, pane, rect, screen_of, theme};

const BOTH: Rules = Rules {
    above: Some(Face::Prompt),
    below: Some(Face::Warning),
};

fn view(buffer: &Buffer) -> TextView<'static> {
    TextView {
        display: buffer.display(),
        buffer: id(),
        revision: 1,
        screen: screen_of(&buffer.display(), ViewTop::default()),
        cursor: DisplayPoint::new(1000, 0),
        cursor_style: CursorStyle::Box,
        others: &[],
        preedit: None,
        fills: &[],
        labels: &[],
        whitespace: false,
        flash: None,
        highlights: None,
        styled: false,
        marks: TextMarks::NONE,
    }
}

/// A pane of `content` in row 2 and, if it has two `rows`, row 3, columns 3 to 12, so from y 48,
/// x 38 to 138, with both rules.
fn ruled(content: PaneContent<'_>, rows: usize) -> Pane<'_> {
    Pane {
        background: Some(Face::Float),
        rules: BOTH,
        ..pane(0, rect(2, 3, rows, 10), 0, content)
    }
}

/// The frame of `panes`, each glyph reaching 40 px above its line's middle and as far below it.
fn scene_of(panes: &[Pane<'_>]) -> Scene {
    let mut atlas = FakeAtlas::default();
    atlas.bitmap = Some(([10, 80], 0, 40));
    let mut scene = Scene::new(LIMITS, theme());
    build(&mut scene, panes, &mut atlas);
    scene
}

/// Each rectangle's place, size and colour, in the order they are drawn.
fn rects(scene: &Scene) -> Vec<([f32; 2], [f32; 2], [f32; 4])> {
    (scene.rects().iter())
        .map(|rect| (rect.position, rect.size, rect.color))
        .collect()
}

fn line(y: f32, face: Face) -> ([f32; 2], [f32; 2], [f32; 4]) {
    ([38.0, y], [100.0, 1.0], theme().foreground(face))
}

fn fill(y: f32, rows: f32, face: Face) -> ([f32; 2], [f32; 2], [f32; 4]) {
    ([38.0, y], [100.0, rows * 20.0], theme().background(face))
}

#[test]
fn the_lines_are_along_the_first_rows_top_and_the_last_rows_bottom() {
    let buffer = Buffer::from_text("t", "x");
    let scene = scene_of(&[ruled(PaneContent::Text(view(&buffer)), 2)]);
    let lines = [
        line(48.0, BOTH.above.unwrap()),
        line(87.0, BOTH.below.unwrap()),
    ];
    assert_eq!(
        rects(&scene),
        [&[fill(48.0, 2.0, Face::Float)], &lines[..]].concat()
    );
    // A pane of lines, such as a gutter, and one row for both.
    let spans = [Span {
        text: "x",
        face: Face::Text,
    }];
    let rows: [&[Span<'_>]; 1] = [&spans];
    let scene = scene_of(&[ruled(PaneContent::Lines(&rows), 1)]);
    let lines = [
        line(48.0, BOTH.above.unwrap()),
        line(67.0, BOTH.below.unwrap()),
    ];
    assert_eq!(
        rects(&scene),
        [&[fill(48.0, 1.0, Face::Float)], &lines[..]].concat()
    );
    // One alone, and none in no cells.
    let alone = Pane {
        rules: Rules {
            below: None,
            ..BOTH
        },
        ..ruled(PaneContent::Lines(&rows), 1)
    };
    assert_eq!(rects(&scene_of(&[alone])).len(), 2);
    assert!(rects(&scene_of(&[ruled(PaneContent::Lines(&rows), 0)])).is_empty());
}

#[test]
fn the_current_lines_band_is_behind_a_line_and_the_other_fills_the_flash_and_the_cursor_over_it() {
    let buffer = Buffer::from_text("t", "xxxx");
    let cells = [rect(2, 3, 1, 10)];
    let selected = [rect(2, 4, 1, 2)];
    let fills: [(Face, &[CellRect]); 3] = [
        (Face::CurrentLine, &cells),
        (Face::DiffAdded, &[]),
        (Face::Region, &selected),
    ];
    let content = TextView {
        cursor: DisplayPoint::new(0, 3),
        fills: &fills,
        flash: Some(FlashFill {
            cells: cells[0],
            strength: 1.0,
        }),
        ..view(&buffer)
    };
    let scene = scene_of(&[ruled(PaneContent::Text(content), 1)]);
    assert_eq!(
        rects(&scene),
        [
            fill(48.0, 1.0, Face::Float),
            fill(48.0, 1.0, Face::CurrentLine),
            line(48.0, BOTH.above.unwrap()),
            line(67.0, BOTH.below.unwrap()),
            ([48.0, 48.0], [20.0, 20.0], theme().background(Face::Region)),
            fill(48.0, 1.0, Face::Flash),
            // The whole cell.
            ([68.0, 48.0], [10.0, 20.0], theme().background(Face::Cursor)),
        ]
    );
}

#[test]
fn a_hollow_cursor_and_the_other_cursors_keep_their_whole_cells() {
    let buffer = Buffer::from_text("t", "xxxx");
    let others = [rect(2, 8, 1, 1)];
    let content = TextView {
        cursor: DisplayPoint::new(0, 0),
        cursor_style: CursorStyle::Hollow,
        others: &others,
        ..view(&buffer)
    };
    let scene = scene_of(&[ruled(PaneContent::Text(content), 1)]);
    let drawn = rects(&scene);
    assert!(drawn.contains(&(
        [88.0, 48.0],
        [10.0, 1.0],
        theme().background(Face::SecondaryCursor)
    )));
    assert!(drawn.contains(&([38.0, 48.0], [10.0, 1.0], theme().background(Face::Cursor))));
    assert!(drawn.contains(&([38.0, 67.0], [10.0, 1.0], theme().background(Face::Cursor))));
}

/// Text, marks that reach past their row, an escape's box and a preedit: none of their pixels is
/// a line's, whichever pane they are in.
#[test]
fn no_text_is_drawn_where_a_line_is() {
    let inside = |scene: &Scene, (top, bottom): (f32, f32)| {
        assert!(!scene.quads().is_empty());
        for quad in scene.quads() {
            let ([_, y], [_, height]) = (quad.position(), quad.size());
            assert!(y >= top && y + height <= bottom, "{y} {height}");
        }
    };
    let buffer = Buffer::from_text("t", "a\u{202e}b\nc\u{202e}d");
    let scene = scene_of(&[ruled(PaneContent::Text(view(&buffer)), 2)]);
    inside(&scene, (49.0, 87.0));
    let boxes: Vec<_> = (rects(&scene).into_iter())
        .filter(|rect| rect.2 == theme().background(Face::Boxed))
        .map(|rect| (rect.0[1], rect.1[1]))
        .collect();
    assert_eq!(boxes, [(49.0, 19.0), (68.0, 19.0)]);
    // A tiled pane of one row of lines, and one told to keep its bands, which keeps none past a
    // line.
    let spans = [Span {
        text: "x\u{202e}",
        face: Face::Text,
    }];
    let rows: [&[Span<'_>]; 1] = [&spans];
    inside(
        &scene_of(&[ruled(PaneContent::Lines(&rows), 1)]),
        (49.0, 67.0),
    );
    let banded = Pane {
        layer: crate::pane::Layer(1),
        bands: Bands {
            above: true,
            below: true,
        },
        ..ruled(PaneContent::Lines(&rows), 1)
    };
    inside(&scene_of(&[banded]), (49.0, 67.0));
    // On the frame's top row too, whose glyphs keep the margin above.
    let top = Pane {
        region: rect(0, 3, 1, 10),
        ..ruled(PaneContent::Lines(&rows), 1)
    };
    inside(&scene_of(&[top]), (9.0, 27.0));
}

#[test]
fn a_preedits_underlines_are_above_the_line_under_them() {
    let buffer = Buffer::from_text("t", "ab");
    // Converting な, the second of two cells of kana.
    let preedit = Preedit::new("かな".into(), Some(3..6));
    let mut editor = Editor::with_buffer(Buffer::from_text("t", "ab"));
    editor.set_view_size(ViewSize::new(1, 10));
    editor.current_mut().0.selections = SelectionSet::new(1);
    editor.set_preedit(Some(preedit.clone()));
    let mut screen = Screen::default();
    editor.screen(&mut screen);
    let content = TextView {
        cursor: DisplayPoint::new(0, 1),
        preedit: Some(&preedit),
        screen: &screen,
        ..view(&buffer)
    };
    let scene = scene_of(&[ruled(PaneContent::Text(content), 1)]);
    let under: Vec<_> = (rects(&scene).into_iter())
        .filter(|rect| rect.2 == theme().background(Face::Preedit) && rect.1[1] < 3.0)
        .map(|rect| (rect.0[1], rect.1[1]))
        .collect();
    assert_eq!(under, [(66.0, 1.0), (65.0, 2.0)]);
}

/// A line under a link, in a view that has styles, stays inside too, on the first row and the last.
#[test]
fn a_links_line_is_inside() {
    let buffer = Buffer::from_text("t", "ab\ncd");
    let mut highlights = Highlights::new(5);
    highlights.replace(0..5, [(5, Some(Highlight::SessionLink))]);
    let content = || TextView {
        highlights: Some(&highlights),
        styled: true,
        ..view(&buffer)
    };
    let under = |rows| {
        let scene = scene_of(&[ruled(PaneContent::Text(content()), rows)]);
        (rects(&scene).into_iter())
            .filter(|rect| rect.2 == theme().foreground(Face::SessionLink))
            .map(|rect| (rect.0[1], rect.1[1]))
            .collect::<Vec<_>>()
    };
    assert_eq!(under(2), [(67.0, 1.0), (86.0, 1.0)]);
    assert_eq!(under(1), [(66.0, 1.0)]);
}

/// A pane that is a line, in rows from 2 and columns 3 to 12.
fn rule(rows: usize) -> Pane<'static> {
    let (ink, above, below) = (Face::Prompt, Face::Text, Face::Float);
    pane(
        0,
        rect(2, 3, rows, 10),
        0,
        PaneContent::Rule { ink, above, below },
    )
}

#[test]
fn a_rule_is_along_its_panes_middle_between_its_two_backgrounds() {
    let scene = scene_of(&[rule(1)]);
    let half = |y, height, face| ([38.0, y], [100.0, height], theme().background(face));
    // 20 px from 48: the line is on a whole pixel, and every pixel is one of theirs.
    assert_eq!(
        rects(&scene),
        [
            half(48.0, 10.0, Face::Text),
            half(59.0, 9.0, Face::Float),
            line(58.0, Face::Prompt),
        ]
    );
    assert!(scene.quads().is_empty());
    // Nothing in no cells.
    assert_eq!(rects(&scene_of(&[rule(0)])), []);
}

/// No glyph of the panes above and below it reaches the line: theirs are cut to their own rows.
#[test]
fn no_text_of_the_panes_beside_a_rule_is_drawn_on_it() {
    let buffer = Buffer::from_text("t", "xxxx");
    let spans = [Span {
        text: "xxxx",
        face: Face::Text,
    }];
    let rows: [&[Span<'_>]; 1] = [&spans];
    let text = Pane {
        background: Some(Face::Float),
        ..pane(1, rect(3, 3, 1, 10), 0, PaneContent::Text(view(&buffer)))
    };
    // A footer's row keeps its band above, within the half under the line.
    let footer = pane(2, rect(5, 3, 1, 10), 0, PaneContent::Lines(&rows));
    let under = Pane {
        id: crate::pane::PaneId(3),
        region: rect(4, 3, 1, 10),
        ..rule(1)
    };
    let scene = scene_of(&[rule(1), text, under, footer]);
    let lines = [58.0, 98.0];
    assert!(!scene.quads().is_empty());
    for glyph in scene.quads() {
        let [top, height] = [glyph.position()[1], glyph.size()[1]];
        for y in lines {
            assert!(top >= y + 1.0 || top + height <= y, "{top} {height} at {y}");
        }
    }
}
