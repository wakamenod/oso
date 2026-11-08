//! What the input method may learn of the cursor: the cell the app last gave, answered only
//! while oso has the IME on (ADR 0069, ADR 0070); and a frame owed to a window that was
//! hidden. Plain values, tested without a window or a clock.
//!
//! Must not depend on: the view or the run loop; nothing here calls AppKit.

use std::time::{Duration, Instant};

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

/// How long a frame owed to a hidden window waits before the window is looked at again.
pub(super) const HIDDEN_RETRY: Duration = Duration::from_millis(150);

/// A frame the renderer skipped because the window was hidden by others. It is owed until the
/// window is visible: looked at again on a short deadline, as the notice that it is visible
/// may never come, and dropped once a notice says it is hidden, which a later notice of it
/// being visible ends with a frame of its own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct HiddenFrame {
    owed: bool,
}

/// What to do about an owed frame before the loop waits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HiddenCheck {
    /// Nothing is owed.
    Nothing,
    /// The window is visible: draw the frame now.
    Draw,
    /// Still hidden: look again at this time.
    LookAgain(Instant),
}

impl HiddenFrame {
    /// The renderer skipped a frame as hidden.
    pub(super) fn skipped(&mut self) {
        self.owed = true;
    }

    /// Whether the window should be looked at before the loop waits.
    pub(super) fn owed(self) -> bool {
        self.owed
    }

    /// A notice says the window is hidden: its notice of being visible will bring the frame.
    pub(super) fn told_hidden(&mut self) {
        self.owed = false;
    }

    /// Before the loop waits, with the window `visible` or not at `now`.
    pub(super) fn check(&mut self, visible: bool, now: Instant) -> HiddenCheck {
        if !self.owed {
            HiddenCheck::Nothing
        } else if visible {
            self.owed = false;
            HiddenCheck::Draw
        } else {
            HiddenCheck::LookAgain(now + HIDDEN_RETRY)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{HIDDEN_RETRY, HiddenCheck, HiddenFrame, ImeGate};
    use crate::window::CursorArea;
    use std::time::Instant;

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
    fn a_frame_skipped_as_hidden_is_drawn_once_the_window_shows_with_no_notice() {
        let start = Instant::now();
        let mut frame = HiddenFrame::default();
        assert_eq!(
            frame.check(true, start),
            HiddenCheck::Nothing,
            "nothing owed"
        );
        frame.skipped();
        // Still hidden: looked at again on the deadline, which wakes the idle loop.
        assert_eq!(
            frame.check(false, start),
            HiddenCheck::LookAgain(start + HIDDEN_RETRY)
        );
        let later = start + HIDDEN_RETRY;
        assert_eq!(
            frame.check(false, later),
            HiddenCheck::LookAgain(later + HIDDEN_RETRY)
        );
        // Visible at the next look, with no notice and no input: drawn, then nothing owed.
        assert_eq!(frame.check(true, later + HIDDEN_RETRY), HiddenCheck::Draw);
        assert_eq!(
            frame.check(true, later + HIDDEN_RETRY),
            HiddenCheck::Nothing
        );
    }

    #[test]
    fn a_notice_that_the_window_is_hidden_stops_the_looking() {
        let start = Instant::now();
        let mut frame = HiddenFrame::default();
        frame.skipped();
        frame.told_hidden();
        assert!(!frame.owed());
        assert_eq!(frame.check(false, start), HiddenCheck::Nothing);
    }
}
