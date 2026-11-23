//! Starts that wait for the configuration's first look (ADR 0173). Until the configuration's
//! thread has looked at the file once, nothing that reads `[sessions]`, `[terminal]` or
//! `[spaces]` as it starts runs with the built-in settings in their place. That covers a hosted
//! session's start, resume, fork or adoption, native or in a terminal, a shell terminal, a new
//! space's naming and directory, and an agent's session tool call that changes something. Each
//! waits here and runs, in the order they came, once that look is done. Nothing is said of it:
//! each just starts a moment later. With no look [`WAIT`] after the first, they run with the
//! settings in force, `*Messages*` says so, and nothing waits from then on. A native resume,
//! a fork or an adoption with no open buffer of its project also waits here for the record of trusted
//! projects' first report, within the same bound (ADR 0165, parts 4a and 4e): it is let go once the
//! record is read, and one kept again as the configuration's look lets it go keeps the
//! deadline it had.
//!
//! Must not depend on: processes or the channel's wire. What waits is carried out by the code
//! that would have carried it out at once.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use super::App;
use crate::channel::Connection;

/// The longest a start waits for the configuration's first look: a look that never comes, as
/// on a hung mount, holds nothing longer.
pub(super) const WAIT: Duration = Duration::from_secs(5);

/// What `*Messages*` says when the wait ran out.
const WAITED_OUT: &str = "The configuration was not read in 5 s: what waited for it started \
                          with the built-in settings";

/// A start that waits, carried out later as it would have been at once.
type Run = Box<dyn FnOnce(&mut App)>;

/// What waits, in the order it came, with the agent's connection each call came from.
pub(super) struct Holding<T> {
    held: VecDeque<(Option<Connection>, T)>,
    /// When what waits runs whether the look came or not: [`WAIT`] after the first.
    until: Option<Instant>,
    /// The wait ran out once: nothing waits from then on.
    waited_out: bool,
}

/// The app's starts that wait.
pub(super) type Held = Holding<Run>;

impl<T> std::fmt::Debug for Holding<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Holding")
            .field("held", &self.held.len())
            .field("until", &self.until)
            .field("waited_out", &self.waited_out)
            .finish()
    }
}

impl<T> Default for Holding<T> {
    fn default() -> Self {
        Self {
            held: VecDeque::new(),
            until: None,
            waited_out: false,
        }
    }
}

impl<T> Holding<T> {
    /// Keeps `item`, from `connection` if an agent's call, at `now`. The first one sets
    /// when the wait ends; later ones never move it.
    pub(super) fn hold(&mut self, connection: Option<Connection>, item: T, now: Instant) {
        self.until.get_or_insert(now + WAIT);
        self.held.push_back((connection, item));
    }

    /// Everything kept, in the order it came. Nothing waits after this until the next hold.
    pub(super) fn take(&mut self) -> Vec<T> {
        self.until = None;
        self.held.drain(..).map(|(_, item)| item).collect()
    }

    /// Keeps `until`, the deadline before a release, for what was kept again while it ran, as
    /// a native resume that still waits for the record of trusted projects is (ADR 0165, part
    /// 4a): it waits no longer than what waited before it.
    pub(super) fn keep_deadline(&mut self, until: Option<Instant>) {
        if let (Some(before), Some(now)) = (until, self.until) {
            self.until = Some(before.min(now));
        }
    }

    /// Whether the wait ends at `now`, with something kept.
    pub(super) fn due(&self, now: Instant) -> bool {
        self.until.is_some_and(|until| until <= now)
    }

    /// Whether anything is kept.
    pub(super) fn is_empty(&self) -> bool {
        self.held.is_empty()
    }

    /// When the wait ends, while something is kept.
    pub(super) fn deadline(&self) -> Option<Instant> {
        self.until
    }

    /// Whether the wait ran out once.
    pub(super) fn waited_out(&self) -> bool {
        self.waited_out
    }

    /// Drops the calls from each connection that is `gone`: they get no reply.
    pub(super) fn drop_calls(&mut self, gone: impl Fn(Connection) -> bool) {
        self.held
            .retain(|(connection, _)| !connection.is_some_and(&gone));
        if self.held.is_empty() {
            self.until = None;
        }
    }
}

impl App {
    /// Whether a start waits now: the configuration's first look is not done, and the wait
    /// has not run out.
    pub(super) fn starts_wait(&self) -> bool {
        !self.configuration.looked() && !self.starting.held.waited_out()
    }

    /// Keeps `run` until the configuration's first look, or until the wait runs out.
    pub(super) fn hold_start(&mut self, run: impl FnOnce(&mut App) + 'static) {
        (self.starting.held).hold(None, Box::new(run), Instant::now());
    }

    /// Keeps `run`, an agent's call from `connection`, as [`App::hold_start`] does. It is
    /// dropped if the connection goes before it runs.
    pub(super) fn hold_call(
        &mut self,
        connection: Connection,
        run: impl FnOnce(&mut App) + 'static,
    ) {
        (self.starting.held).hold(Some(connection), Box::new(run), Instant::now());
    }

    /// Drops the agents' calls that wait from each connection that is `gone`: no reply.
    pub(super) fn drop_held_calls(&mut self, gone: impl Fn(Connection) -> bool) {
        self.starting.held.drop_calls(gone);
    }

    /// Runs what waited, in the order it came: the configuration's first look is done, or
    /// the wait ran out.
    pub(super) fn release_held_starts(&mut self) {
        let until = self.starting.held.deadline();
        let held = self.starting.held.take();
        // Nothing starts in an instance that is going: it would be lost with it (ADR 0133).
        if self.leaving.is_under_way() {
            return;
        }
        for run in held {
            run(self);
        }
        self.starting.held.keep_deadline(until);
    }

    /// Runs what waited with the settings in force once the wait ran out with no look, which
    /// `*Messages*` says once. Before waiting for events.
    pub(super) fn held_starts_due(&mut self) {
        let holding = &self.starting.held;
        if holding.deadline().is_none() || !holding.due(Instant::now()) {
            return;
        }
        self.starting.held.waited_out = true;
        // Else only the record of trusted projects was waited for, which each start judged
        // by it says (ADR 0165, part 4a).
        if !self.configuration.looked() {
            log::warn!("the configuration was not read in time: held starts use the defaults");
            self.editor.note(WAITED_OUT.to_owned());
        }
        self.release_held_starts();
        self.request_redraw();
    }

    /// Whether something waits for the configuration's first look.
    pub(super) fn starts_held(&self) -> bool {
        !self.starting.held.is_empty()
    }

    /// When what waits runs anyway, while something waits.
    pub(super) fn held_starts_deadline(&self) -> Option<Instant> {
        self.starting.held.deadline()
    }
}

#[cfg(test)]
mod tests {
    use super::{Holding, WAIT};
    use crate::channel::Connection;
    use std::time::{Duration, Instant};

    fn connection(number: u64) -> Connection {
        Connection { run: 1, number }
    }

    #[test]
    fn what_waits_is_taken_in_the_order_it_came_once() {
        let mut holding = Holding::default();
        let now = Instant::now();
        holding.hold(None, "claude", now);
        holding.hold(Some(connection(1)), "session_start", now);
        holding.hold(None, "terminal", now);
        assert_eq!(holding.take(), ["claude", "session_start", "terminal"]);
        assert!(holding.take().is_empty());
        assert_eq!(holding.deadline(), None);
    }

    #[test]
    fn the_wait_ends_after_the_first_hold_and_later_ones_do_not_move_it() {
        let mut holding = Holding::default();
        let now = Instant::now();
        assert!(!holding.due(now + WAIT * 2), "nothing kept, nothing due");
        holding.hold(None, 1, now);
        holding.hold(None, 2, now + Duration::from_secs(4));
        assert_eq!(holding.deadline(), Some(now + WAIT));
        assert!(!holding.due(now + WAIT - Duration::from_millis(1)));
        assert!(holding.due(now + WAIT));
    }

    #[test]
    fn what_is_kept_again_in_a_release_keeps_the_deadline_before_it() {
        let mut holding = Holding::default();
        let now = Instant::now();
        holding.hold(None, "resume", now);
        let until = holding.deadline();
        assert_eq!(holding.take(), ["resume"]);
        // Kept again 4 s later, as a resume that still waits for the record.
        holding.hold(None, "resume", now + Duration::from_secs(4));
        holding.keep_deadline(until);
        assert_eq!(holding.deadline(), Some(now + WAIT));
        // Nothing kept again: nothing to wait for.
        let mut empty: Holding<&str> = Holding::default();
        empty.keep_deadline(until);
        assert_eq!(empty.deadline(), None);
    }

    #[test]
    fn a_gone_connections_calls_are_dropped_and_the_rest_kept() {
        let mut holding = Holding::default();
        let now = Instant::now();
        holding.hold(Some(connection(1)), "gone", now);
        holding.hold(None, "person", now);
        holding.hold(Some(connection(2)), "kept", now);
        holding.drop_calls(|of| of == connection(1));
        assert_eq!(holding.take(), ["person", "kept"]);
    }

    #[test]
    fn dropping_every_call_ends_the_wait() {
        let mut holding = Holding::default();
        let now = Instant::now();
        holding.hold(Some(connection(1)), "gone", now);
        holding.drop_calls(|_| true);
        assert_eq!(holding.deadline(), None);
        assert!(!holding.due(now + WAIT));
    }
}
