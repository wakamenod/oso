//! The rules a session thread counts by (ADR 0138, Lifetime and Bounds): when a crashed
//! server starts again, and when it stops for the session; and whether a server floods oso
//! with messages. Both take the time from their caller, so they are tested without waiting.
//!
//! Must not depend on: processes, pipes or the outbox; these only count.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::limits::{
    CRASH_WINDOW, CRASHES_MAX, FLOOD_BYTES_MAX, FLOOD_MESSAGES_MAX, FLOOD_WINDOW, RESTART_DELAYS,
};

/// The crashes within [`CRASH_WINDOW`].
#[derive(Debug, Default)]
pub(super) struct Crashes(VecDeque<Instant>);

impl Crashes {
    /// A crash at `now`: how long to wait before starting again, or `None` when this is the
    /// [`CRASHES_MAX`]th within the window, and the server stops for the session.
    pub(super) fn record(&mut self, now: Instant) -> Option<Duration> {
        while self
            .0
            .front()
            .is_some_and(|&at| now.duration_since(at) >= CRASH_WINDOW)
        {
            self.0.pop_front();
        }
        self.0.push_back(now);
        if self.0.len() >= CRASHES_MAX {
            return None;
        }
        let index = (self.0.len() - 1).min(RESTART_DELAYS.len() - 1);
        Some(RESTART_DELAYS[index])
    }

    /// Forgets them, as `lsp-restart` does.
    pub(super) fn forget(&mut self) {
        self.0.clear();
    }
}

/// The messages and bytes a server sent in each second of the last [`FLOOD_WINDOW`].
#[derive(Debug)]
pub(super) struct Flood {
    since: Instant,
    /// For each second, counted from `since`: which second it holds, its messages and bytes.
    seconds: [(u64, usize, usize); SECONDS],
}

/// The seconds in the window.
const SECONDS: usize = FLOOD_WINDOW.as_secs() as usize;

impl Flood {
    pub(super) fn new(now: Instant) -> Self {
        Self {
            since: now,
            seconds: [(u64::MAX, 0, 0); SECONDS],
        }
    }

    /// A message of `bytes` at `now`. Returns whether the server sent more than the bounds
    /// allow within the window: the seconds it covers, whole, the current one included.
    pub(super) fn count(&mut self, now: Instant, bytes: u64) -> bool {
        let second = now.duration_since(self.since).as_secs();
        let slot = &mut self.seconds[usize::try_from(second % SECONDS as u64).unwrap_or(0)];
        if slot.0 != second {
            *slot = (second, 0, 0);
        }
        slot.1 += 1;
        slot.2 = slot
            .2
            .saturating_add(usize::try_from(bytes).unwrap_or(usize::MAX));
        let within = self
            .seconds
            .iter()
            .filter(|(at, ..)| *at != u64::MAX && second - at < SECONDS as u64);
        let (messages, total) = within.fold((0usize, 0usize), |(m, b), (_, messages, bytes)| {
            (m + messages, b.saturating_add(*bytes))
        });
        messages > FLOOD_MESSAGES_MAX || total > FLOOD_BYTES_MAX
    }
}
