//! The spinner of working marks in `*Sessions*` (ADR 0131, Q11–Q14): which frame shows at an
//! instant, and when the next one is due, only while a spinning row is in view. All rows share
//! one phase, from the instant the spinner started, so one deadline serves them all.
//!
//! Must not depend on: `app`, `dispatch` or a clock: the instants come from the caller.

use std::time::{Duration, Instant};

/// How long a frame shows: 8 frames a second (Q11).
pub const FRAME: Duration = Duration::from_millis(125);

/// The spinner's state: off by `[sessions] spinner = false` (Q14), else running while a
/// spinning row is in view.
#[derive(Debug, Default)]
pub struct Spinner {
    off: bool,
    /// When the spinner started: its first frame.
    epoch: Option<Instant>,
    /// When the next frame is due, while a spinning row is in view.
    due: Option<Instant>,
}

impl Spinner {
    /// `[sessions] spinner` in force: `false` keeps every mark still, and stops the spinner.
    pub fn set_on(&mut self, on: bool) {
        self.off = !on;
        if self.off {
            self.stop();
        }
    }

    /// The frame to draw at `now`, starting with the first when the spinner was stopped, or
    /// `None` while it is off.
    pub fn frame(&mut self, now: Instant) -> Option<usize> {
        if self.off {
            return None;
        }
        let epoch = *self.epoch.get_or_insert(now);
        let frames = now.saturating_duration_since(epoch).as_millis() / FRAME.as_millis();
        Some(usize::try_from(frames).unwrap_or(0))
    }

    /// After the list was drawn at `now`: whether a spinning row is in view. If one is, the next
    /// frame is due at its frame's end; if none is, the spinner stops and sets no deadline.
    pub fn drawn(&mut self, spinning_in_view: bool, now: Instant) {
        match self.epoch.filter(|_| spinning_in_view && !self.off) {
            Some(epoch) => {
                let frames = now.saturating_duration_since(epoch).as_millis() / FRAME.as_millis();
                let next = u32::try_from(frames + 1).unwrap_or(u32::MAX);
                self.due = Some(epoch + FRAME * next);
            }
            None => self.stop(),
        }
    }

    /// Stops: no deadline, and the next start shows the first frame.
    pub fn stop(&mut self) {
        self.epoch = None;
        self.due = None;
    }

    /// When the next frame is due, if the spinner runs.
    pub fn due(&self) -> Option<Instant> {
        self.due
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{FRAME, Spinner};

    #[test]
    fn frames_turn_eight_times_a_second_only_while_a_spinning_row_is_in_view() {
        let t = Instant::now();
        let mut spinner = Spinner::default();
        assert_eq!(spinner.due(), None, "no deadline before anything spins");
        assert_eq!(spinner.frame(t), Some(0));
        spinner.drawn(true, t);
        assert_eq!(spinner.due(), Some(t + FRAME));
        let later = t + FRAME * 3 + Duration::from_millis(10);
        assert_eq!(spinner.frame(later), Some(3));
        spinner.drawn(true, later);
        assert_eq!(spinner.due(), Some(t + FRAME * 4));
        // Nothing spins in view: no deadline, and the next start is the first frame.
        spinner.drawn(false, later);
        assert_eq!(spinner.due(), None);
        assert_eq!(spinner.frame(later), Some(0));
    }

    #[test]
    fn off_keeps_every_mark_still_and_sets_no_deadline() {
        let t = Instant::now();
        let mut spinner = Spinner::default();
        assert_eq!(spinner.frame(t), Some(0));
        spinner.drawn(true, t);
        spinner.set_on(false);
        assert_eq!(spinner.due(), None);
        assert_eq!(spinner.frame(t), None);
        spinner.drawn(true, t);
        assert_eq!(spinner.due(), None);
        spinner.set_on(true);
        assert_eq!(spinner.frame(t + FRAME), Some(0));
    }
}
