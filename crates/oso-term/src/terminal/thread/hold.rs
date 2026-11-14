//! A synchronized update (mode 2026) as the thread keeps it (ADR 0120, step 4a): when the open
//! one began, whether one ended since a snapshot last went, and how long snapshots wait.
//!
//! Must not depend on: the PTY or the engine; the thread tells it what the policy judged.

use std::time::{Duration, Instant};

/// A synchronized update (mode 2026) as the thread keeps it: when the open one began, and
/// whether one ended since a snapshot last went.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::terminal) struct Hold {
    began: Option<Instant>,
    ended: bool,
}

impl Hold {
    /// The program began (true) or ended (false) a hold at `now`. A beginning while one is
    /// open keeps the time it began, so nothing makes a hold last longer.
    pub(in crate::terminal) fn set(&mut self, held: bool, now: Instant) {
        if !held {
            self.began = None;
            self.ended = true;
        } else if self.began.is_none() {
            self.began = Some(now);
        }
    }

    /// A snapshot went. Until then, a hold that ended lets the next one go, even if another
    /// began in the same read: ending and beginning again cannot hold the screen forever.
    pub(in crate::terminal) fn published(&mut self) {
        self.ended = false;
    }

    /// Whether snapshots wait at `now`: a hold is open and younger than `limit`. Past it,
    /// snapshots go as if none were open, until the program ends it and begins another.
    pub(in crate::terminal) fn holds(&self, now: Instant, limit: Duration) -> bool {
        !self.ended && self.left(now, limit).is_some()
    }

    /// How long from `now` the open hold still holds snapshots back, if it does.
    pub(in crate::terminal) fn left(&self, now: Instant, limit: Duration) -> Option<Duration> {
        let age = now.saturating_duration_since(self.began?);
        limit.checked_sub(age).filter(|left| !left.is_zero())
    }
}
