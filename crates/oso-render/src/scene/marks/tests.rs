//! Tests of what a language server's diagnostics draw (ADR 0138): the wavy lines, the
//! strikes, the faded text and a sign column's bars.
//!
//! Must not depend on: `capture`, `paint` or a font backend; a fake text system shapes.

use oso_core::buffer::Buffer;
use oso_core::display_map::DisplayPoint;
use oso_core::frame::CellRect;
use oso_core::syntax::{Highlight, Highlights};
use oso_core::window::ViewTop;

use crate::pane::{CursorStyle, Face, PaneContent, TextMarks, TextView};
use crate::scene::Scene;
use crate::scene::tests::{FakeAtlas, LIMITS, build, id, pane, rect, screen_of, theme};
use crate::shape::MAX_WAVES;

/// What a view is drawn with beyond its marks: fills behind its text, its cursor (out of view
/// if `None`) and its highlights.
type Look<'a> = (
    &'a [(Face, &'a [CellRect])],
    Option<DisplayPoint>,
    Option<&'a Highlights>,
);

/// A view of `buffer` with `marks`, as `look` says.
fn view<'a>(
    buffer: &Buffer,
    marks: TextMarks<'a>,
    (fills, cursor, highlights): Look<'a>,
) -> PaneContent<'a> {
    PaneContent::Text(TextView {
        display: buffer.display(),
        buffer: id(),
        revision: 1,
        screen: screen_of(&buffer.display(), ViewTop::default()),
        cursor: cursor.unwrap_or(DisplayPoint::new(1000, 0)),
        cursor_style: CursorStyle::Box,
        others: &[],
        preedit: None,
        fills,
        labels: &[],
        whitespace: false,
        flash: None,
        highlights,
        marks,
    })
}

fn plain<'a>(buffer: &Buffer, marks: TextMarks<'a>) -> PaneContent<'a> {
    view(buffer, marks, (&[], None, None))
}

/// The frame built of one pane in rows 0 and 1, columns 0 to 19.
fn scene_of(content: PaneContent<'_>) -> Scene {
    let mut scene = Scene::new(LIMITS, theme());
    let panes = [pane(0, rect(0, 0, 2, 20), 0, content)];
    build(&mut scene, &panes, &mut FakeAtlas::default());
    scene
}

#[test]
fn a_wave_is_a_band_at_the_bottom_of_each_row_of_its_cells_in_its_faces_colour() {
    let buffer = Buffer::from_text("t", "let x = 1;\nfn f() {}");
    // Columns 4 to 5 of row 0 an error, and rows 0 and 1 from column 8 a warning's cells.
    let error = [rect(0, 4, 1, 2)];
    let warning = [rect(0, 8, 2, 3)];
    let underlines = [
        (Face::DiagnosticWarning, &warning[..]),
        (Face::DiagnosticError, &error[..]),
    ];
    let marks = TextMarks {
        underlines: &underlines,
        ..TextMarks::NONE
    };
    let scene = scene_of(plain(&buffer, marks));
    let waves: Vec<_> = scene
        .waves()
        .iter()
        .map(|wave| (wave.rect, wave.color, wave.params[..2].to_vec()))
        .collect();
    // 10 x 20 px cells from 8 px in; a line 1 px thick, over a band of 3, a period of 6.
    let (warn, err) = (
        theme().foreground(Face::DiagnosticWarning),
        theme().foreground(Face::DiagnosticError),
    );
    assert_eq!(
        waves,
        [
            ([88.0, 25.0, 30.0, 3.0], warn, vec![1.0, 6.0]),
            ([88.0, 45.0, 30.0, 3.0], warn, vec![1.0, 6.0]),
            ([48.0, 25.0, 20.0, 3.0], err, vec![1.0, 6.0]),
        ],
        "back to front, the most severe last"
    );
    assert_eq!(scene.layers()[0].waves, 0..3);
    assert!(
        scene.waves().iter().all(|wave| wave.bounds
            == [
                wave.rect[0],
                wave.rect[1],
                wave.rect[0] + wave.rect[2],
                wave.rect[1] + wave.rect[3]
            ]),
        "each quad its band"
    );
}

#[test]
fn a_wave_out_of_the_pane_is_cut_to_it_and_waves_stop_at_their_cap() {
    let buffer = Buffer::from_text("t", "abc");
    let wide = [rect(0, 15, 1, 10), rect(5, 0, 1, 3)];
    let underlines = [(Face::DiagnosticInfo, &wide[..])];
    let marks = TextMarks {
        underlines: &underlines,
        ..TextMarks::NONE
    };
    let scene = scene_of(plain(&buffer, marks));
    let rects: Vec<_> = scene.waves().iter().map(|wave| wave.rect).collect();
    assert_eq!(
        rects,
        [[158.0, 25.0, 50.0, 3.0]],
        "to column 20, and row 5 not at all"
    );

    let many = vec![rect(0, 0, 1, 1); MAX_WAVES + 10];
    let underlines = [(Face::DiagnosticError, &many[..])];
    let marks = TextMarks {
        underlines: &underlines,
        ..TextMarks::NONE
    };
    assert_eq!(scene_of(plain(&buffer, marks)).waves().len(), MAX_WAVES);
}

#[test]
fn a_strike_goes_through_the_middle_of_its_cells_in_the_texts_colour() {
    let buffer = Buffer::from_text("t", "old_fn();");
    let struck = [rect(0, 0, 1, 6)];
    let marks = TextMarks {
        struck: &struck,
        ..TextMarks::NONE
    };
    let scene = scene_of(plain(&buffer, marks));
    let rects: Vec<_> = scene
        .rects()
        .iter()
        .map(|rect| (rect.position, rect.size, rect.color))
        .collect();
    assert_eq!(
        rects,
        [([8.0, 18.0], [60.0, 1.0], theme().foreground(Face::Text))]
    );
    assert!(scene.waves().is_empty());
}

/// Each quad's colour, in the order they were built.
fn colours(scene: &Scene) -> Vec<[f32; 4]> {
    scene.quads().iter().map(|quad| quad.color()).collect()
}

#[test]
fn faded_text_keeps_its_syntax_colour_faded_but_on_a_fill_and_under_the_cursor() {
    let text = "let unused = 1;";
    let buffer = Buffer::from_text("t", text);
    let mut highlights = Highlights::new(text.len());
    highlights.replace(0..3, [(3, Some(Highlight::Keyword))]);
    // `let unused`: columns 0 to 9 faded.
    let faded = [rect(0, 0, 1, 10)];
    let marks = TextMarks {
        faded: &faded,
        ..TextMarks::NONE
    };
    // The region over columns 8 and 9, and the cursor on column 4.
    let region = [rect(0, 8, 1, 2)];
    let fills = [(Face::Region, &region[..])];
    let cursor = Some(DisplayPoint::new(0, 4));
    let scene = scene_of(view(&buffer, marks, (&fills, cursor, Some(&highlights))));
    let t = theme();
    let (keyword, text_faded) = (t.faded(Face::SyntaxKeyword), t.faded(Face::Text));
    let (plain_text, cursor) = (t.foreground(Face::Text), t.foreground(Face::Cursor));
    // The fake text system draws blanks too: `let`, the blank, `u` under the cursor, `nus`,
    // `ed` on the region, then ` = 1;` not faded.
    let (k, f, c, p) = (keyword, text_faded, cursor, plain_text);
    assert_eq!(
        colours(&scene),
        [k, k, k, f, c, f, f, f, p, p, p, p, p, p, p]
    );
    assert_ne!(keyword, t.foreground(Face::SyntaxKeyword));
    assert_ne!(text_faded, plain_text);
}

#[test]
fn faded_text_needs_no_highlights() {
    let buffer = Buffer::from_text("t", "ab");
    let faded = [rect(0, 1, 1, 1)];
    let marks = TextMarks {
        faded: &faded,
        ..TextMarks::NONE
    };
    let t = theme();
    assert_eq!(
        colours(&scene_of(plain(&buffer, marks))),
        [t.foreground(Face::Text), t.faded(Face::Text)]
    );
}

#[test]
fn a_sign_is_a_bar_a_third_of_its_cell_at_its_left_in_its_faces_colour() {
    let signs = [
        Some(Face::DiagnosticError),
        None,
        Some(Face::DiagnosticInfo),
        Some(Face::DiagnosticWarning),
    ];
    let mut scene = Scene::new(LIMITS, theme());
    // Three rows: the fourth sign is past the pane.
    let panes = [pane(0, rect(1, 2, 3, 1), 0, PaneContent::Signs(&signs))];
    let system = build(&mut scene, &panes, &mut FakeAtlas::default());
    assert!(system.shaped.is_empty(), "nothing shaped");
    let rects: Vec<_> = scene
        .rects()
        .iter()
        .map(|rect| (rect.position, rect.size, rect.color))
        .collect();
    let t = theme();
    assert_eq!(
        rects,
        [
            (
                [28.0, 28.0],
                [3.0, 20.0],
                t.foreground(Face::DiagnosticError)
            ),
            (
                [28.0, 68.0],
                [3.0, 20.0],
                t.foreground(Face::DiagnosticInfo)
            ),
        ]
    );
    assert!(scene.quads().is_empty());
}
