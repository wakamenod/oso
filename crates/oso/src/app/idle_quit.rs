//! Quitting when idle (ADR 0133, step 5): once the person asked to exit when no terminal is
//! busy (`quit-when-idle`, or `w` to `C-x C-c`'s question), the host waits until none has
//! been for [`HOLD`], with no prompt open, then exits as a confirmed `C-x C-c` would. Which
//! terminals are busy is the editor's: a hosted session with a turn under way or a question
//! open, or another terminal's shell waiting for a job (`Buffer::is_busy`), and git's runs
//! (ADR 0127). A hosted session that is starting,
//! or whose terminal printed within the hold, holds it too: its state is read from its screen,
//! which is untrusted text.
//!
//! Must not depend on: the renderer or the window. [`IdleClock`] is told the time, so it is
//! tested with no clock.

use std::time::{Duration, Instant};

use super::App;
use super::shell::Loop;
use crate::sessions::hosted::Run;

/// How long every terminal stays idle before the exit, so that the moment between two jobs
/// of a line, or a turn's state that flickers, ends nothing.
const HOLD: Duration = Duration::from_secs(2);

/// Since when everything has been idle, while an exit waits for it.
#[derive(Default)]
pub(super) struct IdleClock {
    since: Option<Instant>,
}

impl IdleClock {
    /// Whether the exit is due at `now`, everything being `idle` or not. Not idle, the
    /// clock starts again.
    fn due(&mut self, idle: bool, now: Instant) -> bool {
        if !idle {
            self.since = None;
            return false;
        }
        let since = *self.since.get_or_insert(now);
        now.duration_since(since) >= HOLD
    }

    /// Starts the hold again: a hosted session printed, so its turn may not be over.
    pub(super) fn restart(&mut self) {
        if self.since.is_some() {
            self.since = Some(Instant::now());
        }
    }

    /// When the event loop wakes to look again, if it waits.
    pub(super) fn next_wake(&self) -> Option<Instant> {
        self.since.map(|since| since + HOLD)
    }
}

impl App {
    /// Before the loop waits: exits if an exit waits for the terminals and they have been
    /// idle long enough.
    pub(super) fn quit_if_idle(&mut self, event_loop: &Loop<'_>) {
        // While `oso-restart` moves the terminals, their snapshots stop: none is idle then.
        if !self.editor.quit_when_idle() || self.is_leaving(event_loop) || self.restart_holds() {
            self.idle_quit.since = None;
            return;
        }
        // A session that is still starting has said nothing of a turn yet.
        let starting = (self.hosting.table().iter()).any(|session| session.run == Run::Starting);
        let idle = !starting
            && self.editor.busy_terminals().next().is_none()
            && self.editor.git_runs().next().is_none();
        if !self.idle_quit.due(idle, Instant::now()) {
            return;
        }
        // A prompt open or input held: the clock starts again, and it is tried once more.
        let Some(effects) = self.dispatcher.exit_when_idle(&mut self.editor) else {
            self.idle_quit.since = None;
            return;
        };
        self.idle_quit.since = None;
        self.editor.set_quit_when_idle(false);
        if effects.exit {
            self.leave(event_loop, false);
        }
        self.request_redraw();
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{HOLD, IdleClock};

    #[test]
    fn the_exit_is_due_once_everything_stays_idle_for_the_hold() {
        let start = Instant::now();
        let mut clock = IdleClock::default();
        assert!(!clock.due(false, start));
        assert_eq!(clock.next_wake(), None);
        assert!(!clock.due(true, start));
        assert_eq!(clock.next_wake(), Some(start + HOLD));
        assert!(!clock.due(true, start + HOLD - Duration::from_millis(1)));
        // Busy again for a moment: the hold starts over.
        assert!(!clock.due(false, start + Duration::from_secs(1)));
        let again = start + Duration::from_secs(3);
        assert!(!clock.due(true, again));
        assert!(clock.due(true, again + HOLD));
    }
}
