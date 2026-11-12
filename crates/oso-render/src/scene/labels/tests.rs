//! Tests of a jump's labels over a text pane: the glyphs they hide, their fills and keys, and
//! that they stay in their pane and its layer.
//!
//! Must not depend on: `capture`, `paint` or a font backend; a fake text system shapes.

use oso_core::buffer::Buffer;
use oso_core::display_map::DisplayPoint;
use oso_core::frame::CellRect;
use oso_core::window::ViewTop;
use oso_platform::text::GlyphKey;

use crate::pane::{CursorStyle, Face, PaneContent, Span, TextView};
use crate::scene::Scene;
use crate::scene::tests::{FakeAtlas, LIMITS, build, id, pane, rect, screen_of, theme};

/// A view of `buffer` with `labels` over it, and its cursor at `cursor`.
fn view<'a>(
    buffer: &Buffer,
    cursor: DisplayPoint,
    labels: &'a [(CellRect, Span<'a>)],
) -> PaneContent<'a> {
    PaneContent::Text(TextView {
        display: buffer.display(),
        buffer: id(),
        revision: 1,
        screen: screen_of(&buffer.display(), ViewTop::default()),
        cursor,
        cursor_style: CursorStyle::Box,
        others: &[],
        preedit: None,
        fills: &[],
        labels,
        whitespace: false,
        flash: None,
        highlights: None,
    })
}

/// The cursor out of view.
const AWAY: DisplayPoint = DisplayPoint::new(1000, 0);

fn label(cells: CellRect, text: &str, face: Face) -> (CellRect, Span<'_>) {
    (cells, Span { text, face })
}

/// The characters whose glyphs were looked up: the fake text system's glyph is the character.
fn looked_up(atlas: &FakeAtlas) -> String {
    let char_of = |key: &GlyphKey| char::from_u32(u32::from(key.glyph.0)).unwrap_or('?');
    atlas.looked_up.iter().map(char_of).collect()
}

/// Each quad's cell, as `(row, column)` of the 10 x 20 px grid with a margin of 8 px, and its
/// colour.
fn quads(scene: &Scene) -> Vec<((i32, i32), [f32; 4])> {
    scene
        .quads()
        .iter()
        .map(|quad| {
            let [x, y] = quad.position();
            (((y as i32 - 12) / 20, (x as i32 - 9) / 10), quad.color())
        })
        .collect()
}

/// A quad at `cell` in `face`'s foreground.
fn glyph(cell: (i32, i32), face: Face) -> ((i32, i32), [f32; 4]) {
    (cell, theme().foreground(face))
}

/// Each rectangle's position, size and the face it is the background of, of `faces`.
fn rects(scene: &Scene, faces: &[Face]) -> Vec<([f32; 2], [f32; 2], Option<Face>)> {
    scene
        .rects()
        .iter()
        .map(|r| {
            let face = faces
                .iter()
                .find(|face| theme().background(**face) == r.color);
            (r.position, r.size, face.copied())
        })
        .collect()
}

#[test]
fn a_label_hides_the_glyphs_under_it_and_is_drawn_over_the_cursor_box() {
    let buffer = Buffer::from_text("t", "abcdef");
    let labels = [label(rect(0, 0, 1, 2), "xy", Face::JumpLabel)];
    let panes = [pane(
        0,
        rect(0, 0, 1, 6),
        0,
        view(&buffer, DisplayPoint::new(0, 0), &labels),
    )];
    let (mut scene, mut atlas) = (Scene::new(LIMITS, theme()), FakeAtlas::default());
    build(&mut scene, &panes, &mut atlas);
    assert_eq!(
        looked_up(&atlas),
        "cdefxy",
        "`a` and `b` are not even looked up"
    );
    let faces = [Face::JumpLabel, Face::Cursor];
    let text = |column| glyph((0, column), Face::Text);
    let key = |column| glyph((0, column), Face::JumpLabel);
    assert_eq!(
        quads(&scene),
        [text(2), text(3), text(4), text(5), key(0), key(1)],
        "the keys keep their face on the cursor's cell"
    );
    assert_eq!(
        rects(&scene, &faces),
        [
            ([8.0, 8.0], [10.0, 20.0], Some(Face::Cursor)),
            ([8.0, 8.0], [20.0, 20.0], Some(Face::JumpLabel)),
        ],
        "the label's fill over the cursor box"
    );
}

#[test]
fn a_labels_first_key_and_the_keys_after_it_are_two_runs() {
    let buffer = Buffer::from_text("t", "0123456\n0123456");
    let labels = [
        label(rect(1, 2, 1, 1), "a", Face::JumpLabel),
        label(rect(1, 3, 1, 2), "bc", Face::JumpLabelNext),
    ];
    let panes = [pane(0, rect(0, 0, 2, 7), 0, view(&buffer, AWAY, &labels))];
    let (mut scene, mut atlas) = (Scene::new(LIMITS, theme()), FakeAtlas::default());
    build(&mut scene, &panes, &mut atlas);
    assert_eq!(looked_up(&atlas), "01234560156abc");
    let faces = [Face::JumpLabel, Face::JumpLabelNext];
    assert_eq!(
        quads(&scene).split_off(11),
        [
            glyph((1, 2), Face::JumpLabel),
            glyph((1, 3), Face::JumpLabelNext),
            glyph((1, 4), Face::JumpLabelNext),
        ]
    );
    assert_eq!(
        rects(&scene, &faces),
        [
            ([28.0, 28.0], [10.0, 20.0], Some(Face::JumpLabel)),
            ([38.0, 28.0], [20.0, 20.0], Some(Face::JumpLabelNext)),
        ]
    );
    // The keys are shaped once, and the next frame shapes nothing.
    let system = build(&mut scene, &panes, &mut atlas);
    assert!(system.shaped.is_empty(), "{:?}", system.shaped);
}

#[test]
fn a_wide_glyph_half_under_a_label_is_hidden_whole() {
    let buffer = Buffer::from_text("t", "a\u{3042}b");
    let labels = [label(rect(0, 2, 1, 1), "x", Face::JumpLabel)];
    let panes = [pane(0, rect(0, 0, 1, 6), 0, view(&buffer, AWAY, &labels))];
    let (mut scene, mut atlas) = (Scene::new(LIMITS, theme()), FakeAtlas::default());
    build(&mut scene, &panes, &mut atlas);
    assert_eq!(looked_up(&atlas), "abx");
    assert_eq!(
        quads(&scene),
        [
            glyph((0, 0), Face::Text),
            glyph((0, 3), Face::Text),
            glyph((0, 2), Face::JumpLabel),
        ]
    );
}

#[test]
fn labels_are_cut_to_the_pane_and_stay_in_its_layer() {
    let buffer = Buffer::from_text("t", "abcdef\nabcdef");
    let labels = [
        // Above the pane, then running past its right edge.
        label(rect(0, 0, 1, 1), "x", Face::JumpLabel),
        label(rect(1, 3, 1, 2), "pq", Face::JumpLabelNext),
    ];
    let text = [Span {
        text: "float",
        face: Face::Float,
    }];
    let lines: [&[Span<'_>]; 1] = [&text];
    let panes = [
        pane(0, rect(1, 0, 1, 4), 0, view(&buffer, AWAY, &labels)),
        pane(1, rect(3, 0, 1, 8), 16, PaneContent::Lines(&lines)),
    ];
    let (mut scene, mut atlas) = (Scene::new(LIMITS, theme()), FakeAtlas::default());
    build(&mut scene, &panes, &mut atlas);
    let faces = [Face::JumpLabelNext];
    let tiled = &scene.layers()[0];
    assert_eq!(
        quads(&scene)[tiled.quads.start as usize..tiled.quads.end as usize],
        [
            glyph((1, 0), Face::Text),
            glyph((1, 1), Face::Text),
            glyph((1, 2), Face::Text),
            glyph((1, 3), Face::JumpLabelNext),
        ],
        "no `x` above the pane, and no `q` past it"
    );
    assert_eq!(
        rects(&scene, &faces)[tiled.rects.start as usize..tiled.rects.end as usize],
        [([38.0, 28.0], [10.0, 20.0], Some(Face::JumpLabelNext))]
    );
    assert_eq!(scene.layers()[1].quads.len(), 5, "the float's, in front");
}
