//! What the input method may learn of the cursor, the cell the app last gave, answered only
//! while oso has the IME on (ADR 0069, ADR 0070); and when secure input is held (ADR 0073).
//! Plain values, tested without a window.
//!
//! Must not depend on: the view or the run loop; nothing here calls AppKit.

use crate::window::CursorArea;

/// Whether oso has the IME on, and the cursor's cell the app last gave.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct ImeGate {
    allowed: bool,
    area: Option<CursorArea>,
}

impl ImeGate {
    pub(super) fn allowed(self) -> bool {
        self.allowed
    }

    /// Turns the IME on or off. Off keeps the cell, which is no text, so that the first
    /// composition after a prefix key, or back from a buffer that may hold secrets, opens its
    /// candidate window at the cursor: the app gives the cell again only when it moves.
    pub(super) fn set_allowed(&mut self, allowed: bool) {
        self.allowed = allowed;
    }

    pub(super) fn set_area(&mut self, area: CursorArea) {
        self.area = Some(area);
    }

    /// The cell to answer the input method with: none while the IME is off.
    pub(super) fn area(self) -> Option<CursorArea> {
        if self.allowed { self.area } else { None }
    }
}

/// Secure input: held while the app wants it, oso is the active app and its window has the
/// keyboard (ADR 0073). It says which call to make next, so that exactly one enable is held at
/// a time, a release the system refused is made again before any enable, and nothing enables
/// it once the loop has ended.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct SecureInput {
    wanted: bool,
    active: bool,
    key: bool,
    /// The loop has ended, or the handler panicked: never held again.
    ended: bool,
    held: bool,
    /// A disable the system refused, to be made again first.
    owed: bool,
}

/// A call to the system.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SecureStep {
    Enable,
    Disable,
}

impl SecureInput {
    pub(super) fn held(self) -> bool {
        self.held
    }

    pub(super) fn wanted(self) -> bool {
        self.wanted
    }

    pub(super) fn set_wanted(&mut self, wanted: bool) {
        self.wanted = wanted;
    }

    pub(super) fn set_active(&mut self, active: bool) {
        self.active = active;
    }

    pub(super) fn set_key(&mut self, key: bool) {
        self.key = key;
    }

    /// The loop is ending, or the handler panicked: released, and never held again.
    pub(super) fn release(&mut self) {
        self.ended = true;
    }

    /// The call to make now, if any.
    pub(super) fn next(self) -> Option<SecureStep> {
        if self.owed {
            return Some(SecureStep::Disable);
        }
        let hold = self.wanted && self.active && self.key && !self.ended;
        match (hold, self.held) {
            (true, false) => Some(SecureStep::Enable),
            (false, true) => Some(SecureStep::Disable),
            _ => None,
        }
    }

    /// `step` was made, and the system did or did not accept it.
    pub(super) fn made(&mut self, step: SecureStep, accepted: bool) {
        match step {
            SecureStep::Enable => self.held = accepted,
            SecureStep::Disable => {
                self.held = false;
                self.owed = !accepted;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ImeGate, SecureInput, SecureStep};
    use crate::window::CursorArea;

    const CELL: CursorArea = CursorArea {
        x: 8.0,
        y: 24.0,
        width: 8.0,
        height: 16.0,
    };

    #[test]
    fn the_cell_is_answered_only_while_the_ime_is_on() {
        let mut gate = ImeGate::default();
        gate.set_area(CELL);
        assert_eq!(gate.area(), None, "off at first");
        gate.set_allowed(true);
        assert_eq!(gate.area(), Some(CELL));
        gate.set_allowed(false);
        assert_eq!(gate.area(), None, "off, as after C-x or in a secret buffer");
    }

    #[test]
    fn turning_the_ime_off_and_on_keeps_the_cell() {
        let mut gate = ImeGate::default();
        gate.set_allowed(true);
        gate.set_area(CELL);
        gate.set_allowed(false);
        gate.set_allowed(true);
        assert_eq!(
            gate.area(),
            Some(CELL),
            "the app does not give an unmoved cell again"
        );
    }

    /// Makes every call `secure` asks for, as the system accepting them would, and counts the
    /// enables it holds.
    fn settle(secure: &mut SecureInput, held: &mut i32, accept: bool) {
        for _ in 0..2 {
            let Some(step) = secure.next() else {
                return;
            };
            secure.made(step, accept);
            if accept {
                *held += if step == SecureStep::Enable { 1 } else { -1 };
            }
        }
    }

    #[test]
    fn secure_input_is_held_only_while_wanted_active_and_key() {
        for wanted in [false, true] {
            for active in [false, true] {
                for key in [false, true] {
                    let mut secure = SecureInput::default();
                    let mut held = 0;
                    secure.set_wanted(wanted);
                    secure.set_active(active);
                    secure.set_key(key);
                    settle(&mut secure, &mut held, true);
                    let should = wanted && active && key;
                    assert_eq!(secure.held(), should, "{wanted} {active} {key}");
                    assert_eq!(held, i32::from(should));
                }
            }
        }
    }

    #[test]
    fn enables_and_disables_pair_up_whatever_the_order() {
        let mut secure = SecureInput::default();
        let mut held = 0;
        let mut change = |secure: &mut SecureInput, f: fn(&mut SecureInput)| {
            f(secure);
            settle(secure, &mut held, true);
            held
        };
        change(&mut secure, |s| s.set_active(true));
        change(&mut secure, |s| s.set_key(true));
        assert_eq!(change(&mut secure, |s| s.set_wanted(true)), 1);
        assert_eq!(
            change(&mut secure, |s| s.set_key(true)),
            1,
            "no second enable"
        );
        assert_eq!(
            change(&mut secure, |s| s.set_active(false)),
            0,
            "went behind"
        );
        assert_eq!(change(&mut secure, |s| s.set_active(true)), 1);
        assert_eq!(
            change(&mut secure, |s| s.set_wanted(false)),
            0,
            "another buffer"
        );
    }

    #[test]
    fn nothing_holds_it_again_once_released() {
        let mut secure = SecureInput::default();
        let mut held = 0;
        secure.set_wanted(true);
        secure.set_active(true);
        secure.set_key(true);
        settle(&mut secure, &mut held, true);
        secure.release();
        settle(&mut secure, &mut held, true);
        assert_eq!(held, 0, "released at the end");
        secure.set_active(false);
        secure.set_active(true);
        secure.set_key(true);
        secure.set_wanted(true);
        settle(&mut secure, &mut held, true);
        assert_eq!((held, secure.held()), (0, false), "not again after the end");
    }

    #[test]
    fn a_refused_release_is_made_again_before_any_enable() {
        let mut secure = SecureInput::default();
        secure.set_wanted(true);
        secure.set_active(true);
        secure.set_key(true);
        assert_eq!(secure.next(), Some(SecureStep::Enable));
        secure.made(SecureStep::Enable, true);
        secure.set_wanted(false);
        assert_eq!(secure.next(), Some(SecureStep::Disable));
        secure.made(SecureStep::Disable, false);
        secure.set_wanted(true);
        assert_eq!(
            secure.next(),
            Some(SecureStep::Disable),
            "the owed release first"
        );
        secure.made(SecureStep::Disable, true);
        assert_eq!(secure.next(), Some(SecureStep::Enable), "then held again");
    }

    #[test]
    fn a_refused_enable_is_not_held_and_is_tried_again() {
        let mut secure = SecureInput::default();
        secure.set_wanted(true);
        secure.set_active(true);
        secure.set_key(true);
        secure.made(SecureStep::Enable, false);
        assert!(!secure.held());
        assert_eq!(secure.next(), Some(SecureStep::Enable));
    }
}
