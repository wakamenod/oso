//! The clock of `*Spaces*`: the spinner of working marks (ADR 0131, Q11–Q14) and the blink of
//! waiting rows (ADR 0140, as amended). Which frame and which beat show at an instant, and when
//! the next one is due, only while a spinning or blinking row is in view. All rows share one
//! phase, from the instant the clock started, so one deadline serves them all; a beat is five
//! frames, so that while both run a beat falls on a frame and wakes nothing more.
//!
//! Must not depend on: `app`, `dispatch` or a clock: the instants come from the caller.

use std::time::{Duration, Instant};

/// How long a frame shows: 8 frames a second (Q11).
pub const FRAME: Duration = Duration::from_millis(125);

/// How many frames a beat of the blink lasts.
const FRAMES_A_BEAT: u32 = 5;

/// How long a beat of the blink lasts, lit or not: 625 ms, five frames (ADR 0140, as amended).
pub const BEAT: Duration = Duration::from_millis(125 * FRAMES_A_BEAT as u64);

/// The pane's clock: the spinner, off by `[sessions] spinner = false` (Q14), and the blink, off
/// by `[sessions] blink = false`; running while a spinning or blinking row is in view.
#[derive(Debug, Default)]
pub struct Clock {
    off: bool,
    /// `[sessions] blink = false`: waiting rows stand still.
    blink_off: bool,
    /// When the clock started: its first frame, and its first beat, lit.
    epoch: Option<Instant>,
    /// When the next frame or beat is due, while a spinning or blinking row is in view.
    due: Option<Instant>,
}

impl Clock {
    /// `[sessions] spinner` in force: `false` keeps every mark still, and stops the spinner.
    pub fn set_spinner(&mut self, on: bool) {
        self.off = !on;
        if self.off {
            self.stop();
        }
    }

    /// `[sessions] blink` in force: `false` keeps every waiting row still.
    pub fn set_blink(&mut self, on: bool) {
        self.blink_off = !on;
        if self.blink_off {
            self.stop();
        }
    }

    /// Whether waiting rows blink: `[sessions] blink` is not `false`.
    pub fn blinks(&self) -> bool {
        !self.blink_off
    }

    /// The frames since the clock started, starting it at `now` if it was stopped.
    fn frames(&mut self, now: Instant) -> u128 {
        let epoch = *self.epoch.get_or_insert(now);
        now.saturating_duration_since(epoch).as_millis() / FRAME.as_millis()
    }

    /// The frame to draw at `now`, starting with the first when the clock was stopped, or
    /// `None` while the spinner is off.
    pub fn frame(&mut self, now: Instant) -> Option<usize> {
        if self.off {
            return None;
        }
        Some(usize::try_from(self.frames(now)).unwrap_or(0))
    }

    /// Whether a waiting row is lit at `now`: lit on the first beat, when the clock was
    /// stopped, then on every other. `None` while the blink is off.
    pub fn lit(&mut self, now: Instant) -> Option<bool> {
        if self.blink_off {
            return None;
        }
        let epoch = *self.epoch.get_or_insert(now);
        let beats = now.saturating_duration_since(epoch).as_millis() / BEAT.as_millis();
        Some(beats.is_multiple_of(2))
    }

    /// After the list was drawn at `now`: whether a spinning and a blinking row are in view.
    /// The next frame is due at its frame's end while one spins, else the next beat at its
    /// beat's end while one blinks; while none does, the clock stops and sets no deadline.
    pub fn drawn(&mut self, spinning_in_view: bool, blinking_in_view: bool, now: Instant) {
        let spins = spinning_in_view && !self.off;
        let blinks = blinking_in_view && !self.blink_off;
        let Some(epoch) = self.epoch.filter(|_| spins || blinks) else {
            return self.stop();
        };
        let frames = now.saturating_duration_since(epoch).as_millis() / FRAME.as_millis();
        let step = if spins { 1 } else { u128::from(FRAMES_A_BEAT) };
        let next = u32::try_from((frames / step + 1) * step).unwrap_or(u32::MAX);
        self.due = Some(epoch + FRAME * next);
    }

    /// Stops: no deadline, and the next start shows the first frame and a lit beat.
    pub fn stop(&mut self) {
        self.epoch = None;
        self.due = None;
    }

    /// When the next frame or beat is due, if the clock runs.
    pub fn due(&self) -> Option<Instant> {
        self.due
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{BEAT, Clock, FRAME};

    #[test]
    fn frames_turn_eight_times_a_second_only_while_a_spinning_row_is_in_view() {
        let t = Instant::now();
        let mut spinner = Clock::default();
        assert_eq!(spinner.due(), None, "no deadline before anything spins");
        assert_eq!(spinner.frame(t), Some(0));
        spinner.drawn(true, false, t);
        assert_eq!(spinner.due(), Some(t + FRAME));
        let later = t + FRAME * 3 + Duration::from_millis(10);
        assert_eq!(spinner.frame(later), Some(3));
        spinner.drawn(true, false, later);
        assert_eq!(spinner.due(), Some(t + FRAME * 4));
        // Nothing spins in view: no deadline, and the next start is the first frame.
        spinner.drawn(false, false, later);
        assert_eq!(spinner.due(), None);
        assert_eq!(spinner.frame(later), Some(0));
    }

    #[test]
    fn off_keeps_every_mark_still_and_sets_no_deadline() {
        let t = Instant::now();
        let mut spinner = Clock::default();
        assert_eq!(spinner.frame(t), Some(0));
        spinner.drawn(true, false, t);
        spinner.set_spinner(false);
        assert_eq!(spinner.due(), None);
        assert_eq!(spinner.frame(t), None);
        spinner.drawn(true, false, t);
        assert_eq!(spinner.due(), None);
        spinner.set_spinner(true);
        assert_eq!(spinner.frame(t + FRAME), Some(0));
    }

    #[test]
    fn a_waiting_row_is_lit_on_the_first_beat_then_every_other_one() {
        let t = Instant::now();
        let mut clock = Clock::default();
        assert_eq!(BEAT, FRAME * 5, "a beat falls on a frame");
        assert_eq!(clock.lit(t), Some(true));
        clock.drawn(false, true, t);
        assert_eq!(clock.due(), Some(t + BEAT), "only beats wake it");
        let later = t + BEAT + Duration::from_millis(10);
        assert_eq!(clock.lit(later), Some(false));
        clock.drawn(false, true, later);
        assert_eq!(clock.due(), Some(t + BEAT * 2));
        assert_eq!(clock.lit(t + BEAT * 2), Some(true));
        // Spinning too: frames wake it, and beats fall on them.
        clock.drawn(true, true, later);
        assert_eq!(clock.due(), Some(t + FRAME * 6));
        // Nothing blinks or spins in view: no deadline, and the next start is lit.
        clock.drawn(false, false, later);
        assert_eq!(clock.due(), None);
        assert_eq!(clock.lit(later), Some(true));
    }

    #[test]
    fn blink_off_keeps_waiting_rows_still_and_sets_no_deadline_for_them() {
        let t = Instant::now();
        let mut clock = Clock::default();
        clock.set_blink(false);
        assert_eq!(clock.lit(t), None);
        assert_eq!(clock.frame(t), Some(0));
        clock.drawn(false, true, t);
        assert_eq!(clock.due(), None);
        // The spinner still runs.
        assert_eq!(clock.frame(t), Some(0));
        clock.drawn(true, true, t);
        assert_eq!(clock.due(), Some(t + FRAME));
        clock.set_blink(true);
        assert_eq!(clock.lit(t + FRAME), Some(true));
    }
}
