//! The mouse's input as the terminal's thread serves it (ADR 0120, step 4b-2): it keeps the
//! button held, and encodes each pointer against the modes the program has now, so a report
//! the program no longer wants is dropped here. The wheel's `↑` and `↓` of mode 1007 are not
//! made here: the UI sends them as keys, only to the window it selected, so output that turns
//! reports off cannot make a report into a key for a window the person is not in.
//!
//! Must not depend on: the PTY or the UI; the engine encodes.

use crate::engine::Engine;
use crate::limits;
use crate::snapshot::Mouse;
use crate::snapshot::{Button, Pointer, PointerAction};

/// Encodes `pointer` into `out` and returns the bytes it took, keeping `held` the button the
/// presses and releases served so far hold down.
pub(super) fn serve(
    held: &mut Option<Button>,
    pointer: Pointer,
    engine: &mut impl Engine,
    out: &mut [u8; limits::KEY_BYTES_MAX],
) -> usize {
    match pointer.action {
        PointerAction::Press(button) if !button.is_wheel() => *held = Some(button),
        PointerAction::Release(button) if *held == Some(button) => *held = None,
        _ => {}
    }
    if engine.modes().mouse == Mouse::Off {
        return 0;
    }
    engine.encode_pointer(pointer, *held, out)
}

#[cfg(test)]
mod tests {
    use oso_core::key::Modifiers;

    use super::serve;
    use crate::engine::Engine;
    use crate::engine::ghostty::Ghostty;
    use crate::engine::ghostty::tests::engine;
    use crate::limits;
    use crate::snapshot::{Button, Pointer, PointerAction};

    fn send(e: &mut Ghostty, held: &mut Option<Button>, action: PointerAction) -> Vec<u8> {
        let pointer = Pointer {
            action,
            column: 1,
            row: 1,
            x: 8,
            y: 16,
            modifiers: Modifiers::NONE,
        };
        let mut out = [0; limits::KEY_BYTES_MAX];
        let n = serve(held, pointer, e, &mut out);
        out[..n].to_vec()
    }

    #[test]
    fn the_wheel_sends_only_reports_the_program_asked_for() {
        let mut e = engine(80, 24);
        let mut held = None;
        let up = PointerAction::Press(Button::WheelUp);
        e.feed(b"\x1b[?1049h");
        assert!(e.modes().alternate_scroll, "1007 is on by default");
        assert_eq!(
            send(&mut e, &mut held, up),
            b"",
            "no arrows: the UI sends those"
        );
        e.feed(b"\x1b[?1000h\x1b[?1006h");
        assert_eq!(send(&mut e, &mut held, up), b"\x1b[<64;2;2M");
    }

    #[test]
    fn the_button_held_is_kept_for_drags() {
        let mut e = engine(80, 24);
        e.feed(b"\x1b[?1002h\x1b[?1006h");
        let mut held = None;
        assert_eq!(
            send(&mut e, &mut held, PointerAction::Press(Button::Left)),
            b"\x1b[<0;2;2M"
        );
        assert_eq!(held, Some(Button::Left));
        let wheel = PointerAction::Press(Button::WheelDown);
        send(&mut e, &mut held, wheel);
        assert_eq!(held, Some(Button::Left), "the wheel holds nothing");
        assert_eq!(
            send(&mut e, &mut held, PointerAction::Motion),
            b"\x1b[<32;2;2M"
        );
        send(&mut e, &mut held, PointerAction::Release(Button::Left));
        assert_eq!(held, None);
        assert_eq!(send(&mut e, &mut held, PointerAction::Motion), b"");
    }

    #[test]
    fn a_report_the_program_stopped_asking_for_is_dropped() {
        let mut e = engine(80, 24);
        e.feed(b"\x1b[?1000h\x1b[?1000l");
        let mut held = None;
        let press = PointerAction::Press(Button::Left);
        assert_eq!(send(&mut e, &mut held, press), b"");
    }
}
