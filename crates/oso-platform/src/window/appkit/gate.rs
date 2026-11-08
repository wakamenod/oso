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
/// keyboard (ADR 0073). Each change says whether to enable or disable it, so that exactly one
/// enable is held at a time and every one is released.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct SecureInput {
    wanted: bool,
    active: bool,
    key: bool,
    held: bool,
}

/// What a change asks of the system.
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

    pub(super) fn set_wanted(&mut self, wanted: bool) -> Option<SecureStep> {
        self.wanted = wanted;
        self.settle()
    }

    pub(super) fn set_active(&mut self, active: bool) -> Option<SecureStep> {
        self.active = active;
        self.settle()
    }

    pub(super) fn set_key(&mut self, key: bool) -> Option<SecureStep> {
        self.key = key;
        self.settle()
    }

    /// The loop is ending, or the handler panicked: released, and never held again.
    pub(super) fn release(&mut self) -> Option<SecureStep> {
        self.wanted = false;
        self.active = false;
        self.key = false;
        self.settle()
    }

    /// The system refused to enable it: not held.
    pub(super) fn refused(&mut self) {
        self.held = false;
    }

    fn settle(&mut self) -> Option<SecureStep> {
        let hold = self.wanted && self.active && self.key;
        if hold == self.held {
            return None;
        }
        self.held = hold;
        Some(if hold {
            SecureStep::Enable
        } else {
            SecureStep::Disable
        })
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

    #[test]
    fn secure_input_is_held_only_while_wanted_active_and_key() {
        for wanted in [false, true] {
            for active in [false, true] {
                for key in [false, true] {
                    let mut secure = SecureInput::default();
                    secure.set_wanted(wanted);
                    secure.set_active(active);
                    secure.set_key(key);
                    let should = wanted && active && key;
                    assert_eq!(secure.held(), should, "{wanted} {active} {key}");
                }
            }
        }
    }

    #[test]
    fn enables_and_disables_pair_up_whatever_the_order() {
        let mut secure = SecureInput::default();
        let count = |step: Option<SecureStep>| match step {
            Some(SecureStep::Enable) => 1,
            Some(SecureStep::Disable) => -1,
            None => 0,
        };
        let mut held = count(secure.set_active(true));
        held += count(secure.set_key(true));
        held += count(secure.set_wanted(true));
        assert_eq!(held, 1);
        held += count(secure.set_wanted(true));
        held += count(secure.set_key(true));
        assert_eq!(held, 1, "no second enable");
        held += count(secure.set_active(false));
        assert_eq!(held, 0, "the app went behind");
        held += count(secure.set_active(true));
        assert_eq!(held, 1);
        held += count(secure.set_wanted(false));
        assert_eq!(held, 0, "another buffer selected");
        held += count(secure.set_wanted(true));
        held += count(secure.release());
        assert_eq!(held, 0, "released at the end");
        held += count(secure.set_wanted(true));
        assert_eq!(held, 0, "not again once released, as the loop ended");
    }
}
