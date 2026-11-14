//! Where the mouse goes (ADR 0120, step 4b-2): presses by what holds input, the window, the
//! program's reports and Shift, and the wheel by the program's modes.
//!
//! Must not depend on: a window or a terminal; the routing is pure.

use oso_core::editor::Editor;
use oso_platform::window::MouseButton;
use oso_term::{Modes, Mouse};

use super::{Drag, Pointing, PressGoes, WheelGoes, route_press, wheel_goes, whole};

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
fn the_wheel_goes_to_a_program_that_wants_it_unless_shift_or_held() {
    use WheelGoes::{Arrows, Reports, Scrollback};
    let off = Modes::default();
    let reports = Modes {
        mouse: Mouse::Click,
        ..off
    };
    let alternate = Modes {
        alternate_screen: true,
        alternate_scroll: true,
        ..off
    };
    let alternate_without_1007 = Modes {
        alternate_screen: true,
        ..off
    };
    // (holding, shift, modes, selected, reads a password) → where it goes.
    let table = [
        ((false, false, off, true, false), Scrollback),
        ((false, false, reports, true, false), Reports),
        ((false, false, reports, false, true), Reports),
        ((false, false, alternate, true, false), Arrows),
        ((false, false, alternate, false, false), Scrollback),
        ((false, false, alternate, true, true), Scrollback),
        (
            (false, false, alternate_without_1007, true, false),
            Scrollback,
        ),
        ((false, true, reports, true, false), Scrollback),
        ((false, true, alternate, true, false), Scrollback),
        ((true, false, reports, true, false), Scrollback),
    ];
    for ((holding, shift, modes, selected, password), goes) in table {
        assert_eq!(
            wheel_goes(holding, shift, modes, selected, password),
            goes,
            "holding {holding}, shift {shift}, {modes:?}, selected {selected}, \
             password {password}"
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
