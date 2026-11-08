//! What the input method may learn of the cursor: the cell the app last gave, answered only
//! while oso has the IME on (ADR 0069, ADR 0070). A plain value, tested without a window.
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

#[cfg(test)]
mod tests {
    use super::ImeGate;
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
}
