//! Where the wheel goes (ADR 0120, step 4b-2): by what holds input, Shift, the program's modes,
//! the window and a password being read.
//!
//! Must not depend on: a window or a terminal; the routing is pure.

use oso_term::{Modes, Mouse};

use super::{WheelGoes, wheel_goes};

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
        ((false, false, reports, false, false), Reports),
        ((false, false, reports, true, true), Scrollback),
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
