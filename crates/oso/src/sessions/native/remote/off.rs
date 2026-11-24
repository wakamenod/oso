//! How Remote Control is turned off (ADR 0181): the request that tells claude.ai's side, and
//! when the process is ended, which alone clears the mark.
//!
//! Must not depend on: threads, the stream or the UI; the clock is the caller's.
//!
//! - Idle, the process ends at once. During a turn it ends once the turn is over, [`TURN_WAIT`]
//!   later at most, so that turns that keep coming never keep it.
//! - Not told, as its request was not written, was refused or is not answered within
//!   [`TOLD_WAIT`], the process ends then, whatever runs.
//! - An answer is only taken as told: a forged one leaves the turn's own end.

use std::time::{Duration, Instant};

/// The longest a turn under way keeps the process.
pub const TURN_WAIT: Duration = Duration::from_secs(60);
/// How long the request has to be answered.
pub const TOLD_WAIT: Duration = Duration::from_secs(5);

/// Remote Control being turned off.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Off {
    by: Instant,
    told: Told,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Told {
    /// The request with this id was written, to be answered by then.
    Asked(u64, Instant),
    Yes,
    No,
}

impl Off {
    /// Turned off at `now`, its request's id `id`, if there is a control layer to ask with.
    pub(super) fn begun(id: Option<u64>, now: Instant) -> Self {
        Self {
            by: now + TURN_WAIT,
            told: id.map_or(Told::No, |id| Told::Asked(id, now + TOLD_WAIT)),
        }
    }

    pub(super) fn not_told(&mut self) {
        self.told = Told::No;
    }

    /// The request `id` was answered, with success if `told`.
    pub(super) fn answered(&mut self, id: u64, told: bool) {
        if matches!(self.told, Told::Asked(asked, _) if asked == id) {
            self.told = if told { Told::Yes } else { Told::No };
        }
    }

    /// Whether its request was written and is not answered yet: the process may not have read it.
    pub fn unread(&self) -> bool {
        matches!(self.told, Told::Asked(..))
    }

    /// Whether the process ends at `now`, `idle` or not.
    pub fn ends(&self, idle: bool, now: Instant) -> bool {
        idle || now >= self.deadline() || self.told == Told::No
    }

    /// When the process ends whatever runs.
    pub fn deadline(&self) -> Instant {
        match self.told {
            Told::Asked(_, by) => by.min(self.by),
            Told::Yes | Told::No => self.by,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{Off, TOLD_WAIT, TURN_WAIT};

    const TICK: Duration = Duration::from_millis(1);

    #[test]
    fn an_idle_process_ends_at_once() {
        let now = Instant::now();
        assert!(Off::begun(Some(3), now).ends(true, now));
    }

    #[test]
    fn a_turn_keeps_it_until_its_end_or_a_minute_once_told() {
        let now = Instant::now();
        let mut off = Off::begun(Some(3), now);
        off.answered(3, true);
        assert!(!off.unread());
        assert_eq!(off.deadline(), now + TURN_WAIT);
        assert!(!off.ends(false, now + TURN_WAIT - TICK));
        assert!(off.ends(true, now + TICK));
        assert!(off.ends(false, now + TURN_WAIT));
    }

    #[test]
    fn not_told_it_ends_whatever_runs() {
        let now = Instant::now();
        assert!(Off::begun(None, now).ends(false, now), "no control layer");
        assert!(!Off::begun(None, now).unread() && Off::begun(Some(3), now).unread());
        let mut refused = Off::begun(Some(3), now);
        refused.answered(4, true);
        assert_eq!(refused.deadline(), now + TOLD_WAIT, "another's answer");
        assert!(!refused.ends(false, now + TOLD_WAIT - TICK));
        assert!(refused.ends(false, now + TOLD_WAIT), "not answered");
        refused.answered(3, false);
        assert!(refused.ends(false, now));
        let mut unwritten = Off::begun(Some(3), now);
        unwritten.not_told();
        assert!(unwritten.ends(false, now));
        // Told no once, no later answer takes it back.
        unwritten.answered(3, true);
        assert!(unwritten.ends(false, now));
    }
}
