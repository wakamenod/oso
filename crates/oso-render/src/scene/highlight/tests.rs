//! Tests of the faces plain text takes from its highlights: which glyphs take them, where
//! they give way to the fills, and that a report or text that moves shapes nothing again.
//!
//! Must not depend on: `capture`, `paint` or a font backend; a fake text system shapes.

use std::ops::Range;

use oso_core::buffer::Buffer;
use oso_core::display_map::DisplayPoint;
use oso_core::frame::CellRect;
use oso_core::syntax::{Highlight, Highlights};
use oso_core::window::ViewTop;
use oso_platform::text::{
    CellMetrics, FontId, GlyphBitmap, GlyphId, GlyphKey, ShapedGlyph, TextSystem,
};

use crate::pane::{CursorStyle, Face, FlashFill, PaneContent, TextView};
use crate::rows::tests::Recorder;
use crate::scene::Scene;
use crate::scene::tests::{FakeAtlas, LIMITS, build, id, pane, rect, screen_of, theme};

/// `text`'s highlights: `runs` as ranges with what each is, the rest plain.
fn highlights(text: &str, runs: &[(Range<usize>, Highlight)]) -> Highlights {
    let mut highlights = Highlights::new(text.len());
    for (range, highlight) in runs {
        highlights.replace(range.clone(), [(range.len(), Some(*highlight))]);
    }
    highlights
}

/// What a view of `buffer` at `revision` is drawn with, beyond its text.
#[derive(Default)]
struct Look<'a> {
    highlights: Option<&'a Highlights>,
    fills: &'a [(Face, &'a [CellRect])],
    flash: Option<FlashFill>,
    whitespace: bool,
    cursor: Option<DisplayPoint>,
}

fn view<'a>(buffer: &Buffer, revision: u64, look: Look<'a>) -> PaneContent<'a> {
    PaneContent::Text(TextView {
        display: buffer.display(),
        buffer: id(),
        revision,
        screen: screen_of(&buffer.display(), ViewTop::default()),
        cursor: look.cursor.unwrap_or(DisplayPoint::new(1000, 0)),
        cursor_style: CursorStyle::Box,
        others: &[],
        preedit: None,
        fills: look.fills,
        labels: &[],
        whitespace: look.whitespace,
        flash: look.flash,
        highlights: look.highlights,
        marks: crate::pane::TextMarks::NONE,
    })
}

/// The faces whose foreground each quad is in, in the order they were built.
fn faces(scene: &Scene) -> Vec<Face> {
    let theme = theme();
    let all = [
        Face::Text,
        Face::Escape,
        Face::Cursor,
        Face::Whitespace,
        Face::SyntaxComment,
        Face::SyntaxString,
        Face::SyntaxKeyword,
        Face::DiffAdded,
        Face::SessionAlert,
        Face::SessionQuiet,
    ];
    scene
        .quads()
        .iter()
        .map(|quad| {
            let face = all
                .iter()
                .find(|face| theme.foreground(**face) == quad.color());
            *face.expect("a face of the test's")
        })
        .collect()
}

#[test]
fn plain_text_takes_its_highlights_faces_and_a_report_shapes_nothing_again() {
    let text = "let s = \"s\"; // c";
    let buffer = Buffer::from_text("t", text);
    let mut scene = Scene::new(LIMITS, theme());
    let mut atlas = FakeAtlas::default();
    let region = rect(0, 0, 1, 20);
    build(
        &mut scene,
        &[pane(0, region, 0, view(&buffer, 1, Look::default()))],
        &mut atlas,
    );
    assert!(faces(&scene).iter().all(|face| *face == Face::Text));

    let runs = highlights(
        text,
        &[
            (0..3, Highlight::Keyword),
            (8..11, Highlight::String),
            (13..17, Highlight::Comment),
        ],
    );
    let look = Look {
        highlights: Some(&runs),
        ..Look::default()
    };
    let system = build(
        &mut scene,
        &[pane(0, region, 0, view(&buffer, 1, look))],
        &mut atlas,
    );
    assert!(system.shaped.is_empty(), "{:?}", system.shaped);
    let (k, s, c, t) = (
        Face::SyntaxKeyword,
        Face::SyntaxString,
        Face::SyntaxComment,
        Face::Text,
    );
    assert_eq!(
        faces(&scene),
        [k, k, k, t, t, t, t, t, s, s, s, t, t, c, c, c, c]
    );
}

#[test]
fn escapes_marks_and_the_glyph_under_the_cursor_keep_their_faces() {
    let text = "ab\u{1}c\td";
    let buffer = Buffer::from_text("t", text);
    let runs = highlights(text, &[(0..text.len(), Highlight::Comment)]);
    let look = Look {
        highlights: Some(&runs),
        whitespace: true,
        cursor: Some(DisplayPoint::new(0, 0)),
        ..Look::default()
    };
    let mut scene = Scene::new(LIMITS, theme());
    let content = view(&buffer, 1, look);
    build(
        &mut scene,
        &[pane(0, rect(0, 0, 1, 20), 0, content)],
        &mut FakeAtlas::default(),
    );
    let c = Face::SyntaxComment;
    assert_eq!(
        faces(&scene),
        [
            Face::Cursor,
            c,
            Face::Escape,
            Face::Escape,
            c,
            Face::Whitespace,
            // The tab's blank cell after its mark, which has no ink.
            c,
            c
        ]
    );
}

#[test]
fn text_keeps_its_colour_under_the_fills_and_the_flash_but_not_on_the_band() {
    let text = "abcdefg";
    let buffer = Buffer::from_text("t", text);
    let runs = highlights(text, &[(0..text.len(), Highlight::Keyword)]);
    let (band, region, matched, current, trailing) = (
        [rect(0, 0, 1, 7)],
        [rect(0, 1, 1, 1)],
        [rect(0, 2, 1, 1)],
        [rect(0, 3, 1, 1)],
        [rect(0, 5, 1, 1)],
    );
    let fills: [(Face, &[CellRect]); 5] = [
        (Face::CurrentLine, &band),
        (Face::TrailingWhitespace, &trailing),
        (Face::SearchMatch, &matched),
        (Face::SearchCurrent, &current),
        (Face::Region, &region),
    ];
    let look = Look {
        highlights: Some(&runs),
        fills: &fills,
        flash: Some(FlashFill {
            cells: rect(0, 4, 1, 1),
            strength: 1.0,
        }),
        ..Look::default()
    };
    let mut scene = Scene::new(LIMITS, theme());
    let content = view(&buffer, 1, look);
    build(
        &mut scene,
        &[pane(0, rect(0, 0, 1, 20), 0, content)],
        &mut FakeAtlas::default(),
    );
    let (k, t) = (Face::SyntaxKeyword, Face::Text);
    assert_eq!(faces(&scene), [k, t, t, t, t, t, k]);
}

/// A waiting row of `*Spaces*` on the blink's lit beat (ADR 0140, as amended): its mark and name
/// in their inverse face on their own fill, over the point's band; the other beat draws them as
/// before, and neither beat shapes anything again.
#[test]
fn a_waiting_rows_lit_beat_draws_it_inverse_and_shapes_nothing_again() {
    let text = " ⚠ ask  now";
    let buffer = Buffer::from_text("t", text);
    let named = 1..1 + "⚠ ask".len();
    let age = text.len() - 3..text.len();
    let lit_runs = highlights(
        text,
        &[
            (named.clone(), Highlight::SessionAlertLit),
            (age.clone(), Highlight::SessionQuiet),
        ],
    );
    let mark = 1..1 + '⚠'.len_utf8();
    let unlit_runs = highlights(
        text,
        &[
            (mark, Highlight::SessionAlert),
            (age, Highlight::SessionQuiet),
        ],
    );
    let (point, lit) = ([rect(0, 0, 1, 11)], [rect(0, 1, 1, 5)]);
    let lit_fills: [(Face, &[CellRect]); 2] =
        [(Face::SessionPoint, &point), (Face::SessionAlertLit, &lit)];
    let unlit_fills: [(Face, &[CellRect]); 1] = [(Face::SessionPoint, &point)];
    let mut scene = Scene::new(LIMITS, theme());
    let mut atlas = FakeAtlas::default();
    let region = rect(0, 0, 1, 20);
    let unlit = || Look {
        highlights: Some(&unlit_runs),
        fills: &unlit_fills,
        ..Look::default()
    };
    build(
        &mut scene,
        &[pane(0, region, 0, view(&buffer, 1, unlit()))],
        &mut atlas,
    );
    let (a, q, t) = (Face::SessionAlert, Face::SessionQuiet, Face::Text);
    assert_eq!(faces(&scene), [t, a, t, t, t, t, t, t, q, q, q]);
    let look = Look {
        highlights: Some(&lit_runs),
        fills: &lit_fills,
        ..Look::default()
    };
    let system = build(
        &mut scene,
        &[pane(0, region, 0, view(&buffer, 1, look))],
        &mut atlas,
    );
    assert!(system.shaped.is_empty(), "{:?}", system.shaped);
    // In the text's background colour, as the cursor's glyph is: told by its colour.
    let l = Face::SessionAlertLit;
    let colours: Vec<_> = scene.quads().iter().map(|quad| quad.color()).collect();
    let want: Vec<_> = [t, l, l, l, l, l, t, t, q, q, q]
        .iter()
        .map(|face| theme().foreground(*face))
        .collect();
    assert_eq!(colours, want);
    let system = build(
        &mut scene,
        &[pane(0, region, 0, view(&buffer, 1, unlit()))],
        &mut atlas,
    );
    assert!(system.shaped.is_empty(), "{:?}", system.shaped);
}

/// A review's line of a side, highlighted (ADR 0122, step 9): syntax colours over its added or
/// removed band, and the text's colour under the words refinement marked.
#[test]
fn a_reviews_line_keeps_syntax_colour_on_its_band_but_not_under_refined_words() {
    let text = "+abcdef";
    let buffer = Buffer::from_text("t", text);
    let runs = highlights(
        text,
        &[
            (0..1, Highlight::DiffAdded),
            (1..text.len(), Highlight::Keyword),
        ],
    );
    let (band, refined) = ([rect(0, 0, 1, 7)], [rect(0, 3, 1, 2)]);
    let fills: [(Face, &[CellRect]); 2] =
        [(Face::DiffAdded, &band), (Face::DiffRefineAdded, &refined)];
    let look = Look {
        highlights: Some(&runs),
        fills: &fills,
        ..Look::default()
    };
    let mut scene = Scene::new(LIMITS, theme());
    let content = view(&buffer, 1, look);
    build(
        &mut scene,
        &[pane(0, rect(0, 0, 1, 20), 0, content)],
        &mut FakeAtlas::default(),
    );
    let (a, k, t) = (Face::DiffAdded, Face::SyntaxKeyword, Face::Text);
    assert_eq!(faces(&scene), [a, k, k, t, t, k, k]);
}

#[test]
fn text_that_moves_in_the_buffer_keeps_its_highlights_without_shaping_again() {
    let mut scene = Scene::new(LIMITS, theme());
    let mut atlas = FakeAtlas::default();
    let region = rect(0, 0, 2, 20);
    let before = Buffer::from_text("t", "aa\nbb");
    let runs = highlights("aa\nbb", &[(3..5, Highlight::String)]);
    let look = Look {
        highlights: Some(&runs),
        ..Look::default()
    };
    build(
        &mut scene,
        &[pane(0, region, 0, view(&before, 1, look))],
        &mut atlas,
    );

    // A character typed on the first line moves the second along the buffer.
    let after = Buffer::from_text("t", "xaa\nbb");
    let runs = highlights("xaa\nbb", &[(4..6, Highlight::String)]);
    let look = Look {
        highlights: Some(&runs),
        ..Look::default()
    };
    let system = build(
        &mut scene,
        &[pane(0, region, 0, view(&after, 2, look))],
        &mut atlas,
    );
    assert_eq!(system.shaped, ["xaa"]);
    let (s, t) = (Face::SyntaxString, Face::Text);
    assert_eq!(faces(&scene), [t, t, t, s, s]);
}

/// A text system that shapes `fi` as one glyph over both clusters, and each other character
/// as one glyph.
#[derive(Default)]
struct Ligatures(Recorder);

impl TextSystem for Ligatures {
    fn metrics(&self) -> CellMetrics {
        self.0.metrics()
    }

    fn shape(&mut self, text: &str, out: &mut Vec<ShapedGlyph>) {
        self.0.shape(text, out);
        if let Some(at) = text.find("fi") {
            out.remove(at + 1);
            out[at].advance = 20.0;
            out[at].key = GlyphKey {
                font: FontId(0),
                glyph: GlyphId(0xfb01),
            };
        }
    }

    fn rasterize(&mut self, key: GlyphKey) -> Option<GlyphBitmap> {
        self.0.rasterize(key)
    }
}

#[test]
fn a_ligature_takes_its_first_clusters_face() {
    let text = "fi";
    let buffer = Buffer::from_text("t", text);
    let runs = highlights(
        text,
        &[(0..1, Highlight::Keyword), (1..2, Highlight::String)],
    );
    let look = Look {
        highlights: Some(&runs),
        ..Look::default()
    };
    let content = view(&buffer, 1, look);
    let mut scene = Scene::new(LIMITS, theme());
    let mut system = Ligatures::default();
    let panes = [pane(0, rect(0, 0, 1, 20), 0, content)];
    scene.build(&panes, &mut system, &mut FakeAtlas::default());
    assert_eq!(faces(&scene), [Face::SyntaxKeyword]);
}

#[test]
fn fills_that_meet_keep_the_colour_off_every_cell_they_cover() {
    let text = "abcdefgh";
    let buffer = Buffer::from_text("t", text);
    let runs = highlights(text, &[(0..text.len(), Highlight::Keyword)]);
    let (region, matched, current) = ([rect(0, 1, 1, 3)], [rect(0, 2, 1, 1)], [rect(0, 6, 1, 1)]);
    let fills: [(Face, &[CellRect]); 3] = [
        (Face::SearchMatch, &matched),
        (Face::SearchCurrent, &current),
        (Face::Region, &region),
    ];
    let look = Look {
        highlights: Some(&runs),
        fills: &fills,
        // From inside the region to past its end.
        flash: Some(FlashFill {
            cells: rect(0, 3, 1, 2),
            strength: 1.0,
        }),
        ..Look::default()
    };
    let mut scene = Scene::new(LIMITS, theme());
    let content = view(&buffer, 1, look);
    let panes = [pane(0, rect(0, 0, 1, 20), 0, content)];
    build(&mut scene, &panes, &mut FakeAtlas::default());
    let (k, t) = (Face::SyntaxKeyword, Face::Text);
    assert_eq!(faces(&scene), [k, t, t, t, t, k, t, k]);
}

/// A boxed code point and the wrap mark, inside a highlighted run, keep their own faces, so a
/// highlight never makes a label or a mark read as text (security.md, T3).
#[test]
fn a_box_and_the_wrap_mark_keep_their_faces_inside_a_highlight() {
    let text = "aa\u{202e}a bbbbbb";
    let mut editor = oso_core::editor::Editor::with_buffer(Buffer::from_text("t", text));
    editor.set_view_size(oso_core::window::ViewSize::new(3, 11));
    let id = editor.window().buffer();
    editor.set_wraps(id, true);
    let mut screen = oso_core::window::Screen::default();
    editor.screen(&mut screen);
    let buffer = editor.current().1;
    let runs = highlights(text, &[(0..text.len(), Highlight::Comment)]);
    let content = PaneContent::Text(TextView {
        display: buffer.display(),
        buffer: id,
        revision: buffer.revision(),
        screen: &screen,
        cursor: DisplayPoint::new(1000, 0),
        cursor_style: CursorStyle::Box,
        others: &[],
        preedit: None,
        fills: &[],
        labels: &[],
        whitespace: false,
        flash: None,
        highlights: Some(&runs),
        marks: crate::pane::TextMarks::NONE,
    });
    let mut scene = Scene::new(LIMITS, theme());
    build(
        &mut scene,
        &[pane(0, rect(0, 0, 3, 11), 0, content)],
        &mut FakeAtlas::default(),
    );
    let theme = theme();
    let colours: Vec<_> = scene.quads().iter().map(|quad| quad.color()).collect();
    let count = |face| {
        colours
            .iter()
            .filter(|c| **c == theme.foreground(face))
            .count()
    };
    // The box's label is in the escape colour, which `Boxed` shares.
    assert_eq!(count(Face::Boxed), "U+202E".len());
    assert_eq!(count(Face::WrapMark), 1);
    assert_eq!(count(Face::SyntaxComment) + 6 + 1, colours.len());
}
