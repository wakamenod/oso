//! What the input method may learn of the cursor, the cell the app last gave and where the
//! composition is laid out, answered only while oso has the IME on (ADR 0069,
//! ADR 0070, ADR 0077); when secure input is held (ADR 0073); and a frame owed to a window
//! that was hidden. Plain values, tested without a window or a clock.
//!
//! Must not depend on: the view or the run loop; nothing here calls AppKit.

use std::ops::Range;
use std::time::{Duration, Instant};

use crate::window::{CursorArea, utf16};

/// Whether oso has the IME on, the cursor's cell the app last gave, and where it lays out the
/// composition. No `Debug`: it holds text being typed (ADR 0028).
#[derive(Default)]
pub(super) struct ImeGate {
    allowed: bool,
    area: Option<CursorArea>,
    /// The composition as the app laid it out (ADR 0077): its text, empty with none, its first
    /// cell, and the x of each of its characters from that cell's left, then of its end, in
    /// physical pixels.
    laid_out: String,
    first: Option<CursorArea>,
    positions: Vec<f32>,
}

impl ImeGate {
    pub(super) fn allowed(&self) -> bool {
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

    /// Where the composition `text` is laid out, or none with `None`.
    pub(super) fn set_preedit(&mut self, laid_out: Option<(&str, CursorArea, &[f32])>) {
        self.laid_out.clear();
        self.positions.clear();
        self.first = laid_out.map(|(text, first, positions)| {
            self.laid_out.push_str(text);
            self.positions.extend_from_slice(positions);
            first
        });
    }

    /// The cell to answer the input method with: none while the IME is off.
    pub(super) fn area(&self) -> Option<CursorArea> {
        if self.allowed { self.area } else { None }
    }

    /// Where `units` of the `marked` text are, with the units that answer covers: none while
    /// the IME is off. A range outside the composition, or a composition other than the one
    /// the app laid out, gets the cell, which covers no units (ADR 0077).
    pub(super) fn rect(
        &self,
        marked: &str,
        units: Range<usize>,
    ) -> Option<(CursorArea, Option<Range<usize>>)> {
        let area = self.area()?;
        let Some(first) = self.first.filter(|_| self.laid_out == marked) else {
            return Some((area, None));
        };
        let at = utf16::locate(marked, units);
        let (Some(&start), Some(&end)) = (
            self.positions.get(at.chars.start),
            self.positions.get(at.chars.end),
        ) else {
            return Some((area, None));
        };
        if at.past_end {
            return Some((area, None));
        }
        let placed = CursorArea {
            x: first.x + start,
            width: end - start,
            ..first
        };
        Some((placed, Some(at.units)))
    }
}

/// Whether AppKit reads the view's input context again now, rather than after the next event
/// it handles, as it does by itself: only when the IME goes off for a buffer that may hold
/// secrets (`secret`, as secure input is wanted for it), and the flip did not come in an event
/// AppKit dispatched. Then the input method is not left active over that buffer until a key
/// comes. Any other flip, as a file that is read-only while it loads, waits for the next event:
/// the view takes no text while the IME is off, and AppKit's update costs time on every flip.
pub(super) fn reads_context_now(allowed: bool, secret: bool, in_event: bool) -> bool {
    !allowed && secret && !in_event
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
        HIDDEN_RETRY, HiddenCheck, HiddenFrame, ImeGate, SecureInput, SecureStep, reads_context_now,
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
    fn a_range_of_the_composition_is_where_the_app_laid_it_out() {
        let mut gate = ImeGate::default();
        gate.set_allowed(true);
        gate.set_area(CELL);
        // Scrolled since the cell was given: the composition starts two rows lower.
        let first = CursorArea { y: 56.0, ..CELL };
        // "かa😀": a wide letter, a narrow one, and an emoji of two cells and two units.
        let text = "かa😀";
        gate.set_preedit(Some((text, first, &[0.0, 16.0, 24.0, 40.0])));
        let at = |x: f32, width: f32, units| Some((CursorArea { x, width, ..first }, Some(units)));
        assert_eq!(
            gate.rect(text, 0..1),
            at(8.0, 16.0, 0..1),
            "the first letter"
        );
        assert_eq!(
            gate.rect(text, 1..4),
            at(24.0, 24.0, 1..4),
            "the segment after it"
        );
        assert_eq!(
            gate.rect(text, 4..4),
            at(48.0, 0.0, 4..4),
            "the caret at the end"
        );
        assert_eq!(
            gate.rect(text, 3..4),
            at(48.0, 0.0, 4..4),
            "inside the emoji"
        );
        assert_eq!(gate.rect(text, 1..99), at(24.0, 24.0, 1..4), "cut to it");
        assert_eq!(
            gate.rect(text, 5..6),
            Some((CELL, None)),
            "past the composition"
        );
        assert_eq!(
            gate.rect("かb😀", 0..1),
            Some((CELL, None)),
            "another composition"
        );
        assert_eq!(gate.rect("", 0..0), Some((CELL, None)), "none marked");
        gate.set_allowed(false);
        assert_eq!(gate.rect(text, 0..1), None, "off");
        gate.set_allowed(true);
        gate.set_preedit(None);
        assert_eq!(gate.rect(text, 0..1), Some((CELL, None)), "none laid out");
    }

    #[test]
    fn no_range_reaches_past_the_positions() {
        let mut gate = ImeGate::default();
        gate.set_allowed(true);
        gate.set_area(CELL);
        let text = "a😀か";
        // Fewer positions than characters, as no app gives, must still answer in bounds.
        for given in 0..=4 {
            gate.set_preedit(Some((text, CELL, &[0.0, 8.0, 24.0, 40.0][..given])));
            for start in (0..=6).chain([usize::MAX]) {
                for end in (0..=6).chain([usize::MAX]) {
                    let (area, units) = gate.rect(text, start..end).unwrap();
                    assert!(area.width >= 0.0 && area.x >= CELL.x, "{start}..{end}");
                    assert!(units.is_none_or(|units| units.end <= 4), "{start}..{end}");
                }
            }
        }
    }

    #[test]
    fn only_the_ime_going_off_for_a_secret_buffer_outside_an_event_reads_the_context_now() {
        assert!(
            reads_context_now(false, true, false),
            "a secret buffer, from the socket"
        );
        assert!(
            !reads_context_now(false, false, false),
            "a file read-only while it loads"
        );
        assert!(
            !reads_context_now(true, false, false),
            "back on after a load"
        );
        assert!(
            !reads_context_now(true, true, false),
            "on, if ever, for a secret buffer"
        );
        assert!(
            !reads_context_now(false, true, true),
            "AppKit reads it after the key"
        );
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
