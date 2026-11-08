//! What the input method may learn of the cursor, the cell the app last gave, answered only
//! while oso has the IME on (ADR 0069, ADR 0070); when secure input is held (ADR 0073); a
//! frame owed to a window that was hidden; and a live resize, whose frames are drawn at each
//! step (ADR 0086). Plain values, tested without a window or a clock.
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

/// Secure input: held while the app wants it, oso is the active app and its window has the
/// keyboard, until the loop ends (ADR 0073). It counts the enables the system holds for oso
/// and says which call to make next:
///
/// - one enable is held while it is wanted, and an enable the system refused is asked for
///   again at the next settle;
/// - a release the system refused is asked for again, and never stands in the way of an
///   enable wanted now, as the system counts them;
/// - every enable is released once it is no longer wanted, and nothing enables it again once
///   the loop has ended.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct SecureInput {
    wanted: bool,
    active: bool,
    key: bool,
    /// The loop has ended, or the handler panicked.
    ended: bool,
    /// Enables the system accepted and has not been asked to release.
    enables: u32,
}

/// A call to the system.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SecureStep {
    Enable,
    Disable,
}

impl SecureInput {
    pub(super) fn held(self) -> bool {
        self.enables > 0
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
        let hold = self.wanted && self.active && self.key && !self.ended;
        match (hold, self.enables) {
            (true, 0) => Some(SecureStep::Enable),
            (true, 1) | (false, 0) => None,
            // More than one, after a release the system refused, or any once not wanted.
            _ => Some(SecureStep::Disable),
        }
    }

    /// `step` was made, and the system accepted it or not.
    fn made(&mut self, step: SecureStep, accepted: bool) {
        match (step, accepted) {
            (SecureStep::Enable, true) => self.enables += 1,
            (SecureStep::Disable, true) => self.enables = self.enables.saturating_sub(1),
            (_, false) => {}
        }
    }

    /// Makes the calls this asks for, through `call`, which gets the step and the state as it
    /// will be if the system accepts, to keep before the call, and says whether the system
    /// accepted. It stops at a refusal, which the next settle asks for again.
    pub(super) fn settle(&mut self, mut call: impl FnMut(SecureStep, SecureInput) -> bool) {
        // At most two calls: a release still owed, then the one the change asks for.
        for _ in 0..2 {
            let Some(step) = self.next() else {
                return;
            };
            let mut hoped = *self;
            hoped.made(step, true);
            let accepted = call(step, hoped);
            self.made(step, accepted);
            if !accepted {
                return;
            }
        }
    }
}

/// How long a frame owed to a hidden window waits before the window is looked at again.
pub(super) const HIDDEN_RETRY: Duration = Duration::from_millis(150);

/// A live resize, as the person drags the window's edge: while one runs, each step of it is
/// drawn at once, with its new size in the same Core Animation transaction, so that no frame
/// is stretched to a size it was not drawn at. It counts the steps and the frames drawn for
/// them, for the dev socket's check.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct LiveResize {
    live: bool,
    steps: u32,
    frames: u32,
}

impl LiveResize {
    /// A live resize starts: its counts start again.
    pub(super) fn start(&mut self) {
        *self = Self {
            live: true,
            ..Self::default()
        };
    }

    /// The live resize ends; its counts stay until the next.
    pub(super) fn end(&mut self) {
        self.live = false;
    }

    /// Whether a step's frame is drawn at once.
    #[cfg(test)]
    fn live(self) -> bool {
        self.live
    }

    /// A step of the live resize: the window took a new size. Returns whether its frame is
    /// drawn at once.
    pub(super) fn step(&mut self) -> bool {
        if self.live {
            self.steps = self.steps.saturating_add(1);
        }
        self.live
    }

    /// A frame was drawn for a step.
    pub(super) fn drew(&mut self) {
        if self.live {
            self.frames = self.frames.saturating_add(1);
        }
    }

    /// The steps and the frames drawn for them, of this live resize or the last.
    pub(super) fn counts(self) -> (u32, u32) {
        (self.steps, self.frames)
    }
}

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
    use super::{
        HIDDEN_RETRY, HiddenCheck, HiddenFrame, ImeGate, LiveResize, SecureInput, SecureStep,
    };
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
    fn only_a_live_resize_draws_its_steps_at_once_and_counts_them() {
        let mut resize = LiveResize::default();
        assert!(
            !resize.step(),
            "a size the app asked for waits for the loop"
        );
        resize.drew();
        assert_eq!(resize.counts(), (0, 0));
        resize.start();
        assert!(resize.live());
        assert!(resize.step() && resize.step());
        resize.drew();
        assert_eq!(resize.counts(), (2, 1), "a step drawn, one not");
        resize.end();
        assert!(!resize.live() && !resize.step());
        assert_eq!(resize.counts(), (2, 1), "kept until the next");
        resize.start();
        assert_eq!(resize.counts(), (0, 0));
    }

    /// The system's own count of enables, behind a `settle` that accepts as told.
    fn settle(secure: &mut SecureInput, system: &mut i32, accept: bool) {
        secure.settle(|step, _| {
            if accept {
                *system += if step == SecureStep::Enable { 1 } else { -1 };
            }
            accept
        });
    }

    #[test]
    fn secure_input_is_held_only_while_wanted_active_key_and_not_ended() {
        for wanted in [false, true] {
            for active in [false, true] {
                for key in [false, true] {
                    for ended in [false, true] {
                        let mut secure = SecureInput::default();
                        let mut system = 0;
                        secure.set_wanted(wanted);
                        secure.set_active(active);
                        secure.set_key(key);
                        if ended {
                            secure.release();
                        }
                        settle(&mut secure, &mut system, true);
                        let should = wanted && active && key && !ended;
                        assert_eq!(secure.held(), should, "{wanted} {active} {key} {ended}");
                        assert_eq!(system, i32::from(should), "the system's count");
                    }
                }
            }
        }
    }

    #[test]
    fn enables_and_disables_pair_up_whatever_the_order() {
        let mut secure = SecureInput::default();
        let mut system = 0;
        let mut change = |secure: &mut SecureInput, f: fn(&mut SecureInput)| {
            f(secure);
            settle(secure, &mut system, true);
            system
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
        let mut system = 0;
        secure.set_wanted(true);
        secure.set_active(true);
        secure.set_key(true);
        settle(&mut secure, &mut system, true);
        secure.release();
        settle(&mut secure, &mut system, true);
        assert_eq!(system, 0, "released at the end");
        secure.set_active(false);
        secure.set_active(true);
        secure.set_key(true);
        secure.set_wanted(true);
        settle(&mut secure, &mut system, true);
        assert_eq!(
            (system, secure.held()),
            (0, false),
            "not again after the end"
        );
    }

    #[test]
    fn a_refused_release_never_blocks_a_later_enable_and_the_books_balance() {
        let mut secure = SecureInput::default();
        let mut system = 0;
        secure.set_wanted(true);
        secure.set_active(true);
        secure.set_key(true);
        settle(&mut secure, &mut system, true);
        // Another buffer, and the system refuses the release, every time.
        secure.set_wanted(false);
        settle(&mut secure, &mut system, false);
        settle(&mut secure, &mut system, false);
        assert_eq!(system, 1, "the system still holds it");
        // A secret buffer again: wanted now, and held, with no enable on top.
        secure.set_wanted(true);
        assert_eq!(secure.next(), None, "the one held serves");
        assert!(secure.held());
        // Not wanted, and now the system accepts: back to none.
        secure.set_wanted(false);
        settle(&mut secure, &mut system, true);
        assert_eq!((system, secure.held()), (0, false));
    }

    #[test]
    fn a_refused_enable_is_asked_for_again_at_the_next_settle() {
        let mut secure = SecureInput::default();
        let mut system = 0;
        secure.set_wanted(true);
        secure.set_active(true);
        secure.set_key(true);
        settle(&mut secure, &mut system, false);
        assert!(!secure.held());
        // Nothing changed, as at the next turn of the loop: asked again, and now accepted.
        settle(&mut secure, &mut system, true);
        assert_eq!((system, secure.held()), (1, true));
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
