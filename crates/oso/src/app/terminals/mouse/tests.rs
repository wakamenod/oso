//! Where the mouse goes (ADR 0120, step 4b-2): presses by what holds input, the window, the
//! program's reports and Shift; and how a drag ends.
//!
//! Must not depend on: a window or a terminal; the routing is pure.

use oso_core::editor::Editor;
use oso_platform::window::MouseButton;

use super::{Drag, Pointing, PressGoes, route_press, whole};

#[test]
fn the_press_routing_table() {
    use PressGoes::{Dropped, Nothing, Report, SelectWindow, Selection};
    // (holding, selected, terminal, in_text, reports, shift, left) → where it goes.
    let table = [
        ((true, true, true, true, true, false, true), Dropped),
        ((true, false, true, true, false, false, true), Dropped),
        ((true, true, false, true, false, false, true), Dropped),
        ((false, false, true, true, true, false, true), SelectWindow),
        (
            (false, false, false, true, false, false, true),
            SelectWindow,
        ),
        ((false, false, true, true, false, true, true), SelectWindow),
        ((false, true, false, true, false, false, true), Nothing),
        ((false, true, true, false, true, false, true), Nothing),
        ((false, true, true, true, true, false, true), Report),
        ((false, true, true, true, true, false, false), Report),
        ((false, true, true, true, true, true, true), Selection),
        ((false, true, true, true, false, false, true), Selection),
        ((false, true, true, true, false, true, true), Selection),
        ((false, true, true, true, false, false, false), Nothing),
        ((false, true, true, true, true, true, false), Nothing),
    ];
    for ((holding, selected, terminal, in_text, reports, shift, left), goes) in table {
        assert_eq!(
            route_press(holding, selected, terminal, in_text, reports, shift, left),
            goes,
            "holding {holding}, selected {selected}, terminal {terminal}, in text {in_text}, \
             reports {reports}, shift {shift}, left {left}"
        );
    }
}

#[test]
fn a_place_rounds_down_and_never_below_zero() {
    assert_eq!(whole(3.9), 3);
    assert_eq!(whole(-0.5), 0);
    assert_eq!(whole(f64::NAN), 0);
    assert_eq!(whole(f64::INFINITY), 0);
}

#[test]
fn a_drag_ended_early_leaves_its_release_nothing_to_copy() {
    let editor = Editor::default();
    let (window, buffer) = (editor.tiled().0.id(), editor.tiled().0.buffer());
    let mut pointing = Pointing::default();
    // A press begins a selection; the window loses the focus before the release.
    pointing.begin(MouseButton::Left, Drag::Select { window, buffer });
    assert_eq!(
        pointing.take_drag(),
        Some((MouseButton::Left, Drag::Select { window, buffer }))
    );
    assert_eq!(
        pointing.released(MouseButton::Left),
        None,
        "nothing to copy"
    );
    // The next press selects only the window; its release copies nothing either.
    pointing.begin(MouseButton::Left, Drag::Nothing);
    assert_eq!(
        pointing.released(MouseButton::Right),
        None,
        "another button"
    );
    assert_eq!(pointing.released(MouseButton::Left), Some(Drag::Nothing));
}
