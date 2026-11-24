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

#[test]
#[ignore = "needs a GPU"]
fn a_rule_along_a_panes_middle_is_one_line_between_two_backgrounds() {
    let mut rule = pane(0, (2, 0, 1, 6), 0, None, &[]);
    rule.content = PaneContent::Rule {
        ink: Face::Warning,
        above: Face::Modeline,
        below: Face::Float,
    };
    let pixels = draw(&[rule]);
    let background = |face| theme().background(face);
    for x in [8, 40, 67] {
        assert_is(&pixels, (x, 48), background(Face::Modeline), "over it");
        assert_is(&pixels, (x, 57), background(Face::Modeline), "over it");
        assert_is(
            &pixels,
            (x, 58),
            theme().foreground(Face::Warning),
            "the line",
        );
        assert_is(&pixels, (x, 59), background(Face::Float), "under it");
        assert_is(&pixels, (x, 67), background(Face::Float), "under it");
    }
    assert_is(&pixels, (40, 47), PAPER(), "nothing past the pane");
    assert_is(&pixels, (40, 68), PAPER(), "nothing past the pane");
}
