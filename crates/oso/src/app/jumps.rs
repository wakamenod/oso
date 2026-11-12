//! The time a jump's string is read for (ADR 0116): after each input while a jump reads its
//! string, its labels show once `[jump] timeout` passes with no other. What a jump finds and
//! labels is the editor's; this only keeps time.
//!
//! Must not depend on: the renderer or the window. [`JumpTimer`] is told the time, so it is
//! tested with no clock.

use std::time::{Duration, Instant};

use oso_core::jump::Stage;

use super::App;

/// How long a jump reads its string after the last key, as avy's `avy-timeout-seconds`.
pub(super) const DEFAULT_TIMEOUT: Duration = Duration::from_millis(500);

/// How early a wake may come and still count, as the flash's (`flash::EARLY`).
const EARLY: Duration = Duration::from_millis(1);

/// When a jump reading its string shows its labels, if one does.
#[derive(Debug)]
pub(super) struct JumpTimer {
    timeout: Duration,
    due: Option<Instant>,
}

impl Default for JumpTimer {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_TIMEOUT,
            due: None,
        }
    }
}

impl JumpTimer {
    /// Sets how long a string is read after its last key, as `[jump] timeout` gives it.
    pub(super) fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    /// After an input at `now`: the time starts again while a jump `reading` its string is
    /// under way, and stops otherwise.
    fn after_input(&mut self, reading: bool, now: Instant) {
        self.due = reading.then(|| now + self.timeout);
    }

    /// When the loop must wake for it.
    pub(super) fn next_wake(&self) -> Option<Instant> {
        self.due
    }

    /// Whether its time has come at `now`, which it then forgets.
    fn take_due(&mut self, now: Instant) -> bool {
        match self.due {
            Some(due) if now + EARLY >= due => {
                self.due = None;
                true
            }
            _ => false,
        }
    }
}

impl App {
    /// After an input the dispatcher handled, if one came: the jump's time starts again.
    pub(super) fn time_jump(&mut self, input: bool) {
        if input {
            let reading = (self.editor.jump()).is_some_and(|jump| jump.stage() == Stage::Reading);
            self.jump_timer.after_input(reading, Instant::now());
        }
    }

    /// Shows a jump's labels once its time has passed. A string not begun waits for a key.
    pub(super) fn show_jump_labels_if_due(&mut self) {
        if self.jump_timer.take_due(Instant::now()) {
            self.editor.jump_show_labels();
            self.request_redraw();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_TIMEOUT, JumpTimer};
    use std::time::{Duration, Instant};

    #[test]
    fn each_key_while_reading_starts_the_time_again_and_others_stop_it() {
        let start = Instant::now();
        let mut timer = JumpTimer::default();
        assert_eq!(timer.next_wake(), None);
        timer.after_input(true, start);
        assert_eq!(timer.next_wake(), Some(start + DEFAULT_TIMEOUT));
        let later = start + Duration::from_millis(300);
        timer.after_input(true, later);
        assert!(!timer.take_due(start + DEFAULT_TIMEOUT));
        assert!(timer.take_due(later + DEFAULT_TIMEOUT));
        assert_eq!(timer.next_wake(), None, "once");
        timer.after_input(true, start);
        timer.after_input(false, start);
        assert_eq!(timer.next_wake(), None);
        timer.set_timeout(Duration::from_secs(2));
        timer.after_input(true, start);
        assert_eq!(timer.next_wake(), Some(start + Duration::from_secs(2)));
    }
}
