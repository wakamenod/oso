//! The lines along a pane's edges (ADR 0170), in real pixels.
//!
//! Must not depend on: a font backend; the glyphs are solid blocks that reach past their rows.

use super::*;
use crate::pane::Rules;

#[test]
#[ignore = "needs a GPU"]
fn a_ruled_panes_text_is_between_its_lines_and_no_other_panes_is_drawn_there() {
    let text = [Span {
        text: "xxxxxx",
        face: Face::Text,
    }];
    let rows: [&[Span<'_>]; 1] = [&text];
    // Row 2 is y 48..68, columns 0 to 5 are x 8..68. The row above it is full of glyphs that reach
    // below theirs, and a pane's in front says something on it, as a composer's placeholder does.
    let rules = Rules {
        above: Some(Face::Warning),
        below: Some(Face::Warning),
    };
    let ruled = |pane| Pane { rules, ..pane };
    let panes = [
        pane(0, (1, 0, 1, 6), 0, Some(Face::Text), &rows),
        ruled(pane(1, (2, 0, 1, 2), 0, Some(Face::Float), &rows)),
        ruled(pane(2, (2, 2, 1, 4), 0, Some(Face::Float), &[])),
        ruled(pane(3, (2, 3, 1, 3), 1, None, &rows)),
    ];
    let pixels = draw(&panes);
    let line = theme().foreground(Face::Warning);
    for x in 8..68 {
        assert_is(&pixels, (x, 48), line, "the line above");
        assert_is(&pixels, (x, 67), line, "the line below");
    }
    assert_is(&pixels, (12, 47), INK(), "the row above's own text");
    assert_is(&pixels, (12, 49), INK(), "text just inside");
    assert_is(&pixels, (12, 66), INK(), "text just inside");
    assert_is(
        &pixels,
        (30, 58),
        theme().background(Face::Float),
        "no text",
    );
    assert_is(&pixels, (42, 49), INK(), "the one in front");
    assert_is(&pixels, (42, 68), PAPER(), "nothing past the pane");
}
