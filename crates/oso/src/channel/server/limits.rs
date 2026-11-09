//! The agent channel server's limits (ADR 0085): who may ask the person and when, the slots
//! open connections take, and a connection's requests a second.
//!
//! Must not depend on: sockets or the UI; these only count.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use super::{MAX_ASKS, MAX_RATE, REFUSED_FOR, Shared};

/// Who may ask the person, and when.
#[derive(Debug, Default)]
pub(super) struct Gate {
    pub(super) accepts: VecDeque<Instant>,
    asks: VecDeque<Instant>,
    refused_until: Option<Instant>,
    waiting: bool,
    /// When closing silent connections was last noted.
    pub(super) silent_noted: Option<Instant>,
}

impl Gate {
    /// Forgets what is older than a minute.
    pub(super) fn prune(&mut self, now: Instant) {
        let minute = Duration::from_secs(60);
        for times in [&mut self.accepts, &mut self.asks] {
            while times
                .front()
                .is_some_and(|&at| now.duration_since(at) >= minute)
            {
                times.pop_front();
            }
        }
    }

    /// Whether a connection may ask now, and if so notes that it does.
    pub(super) fn may_ask(&mut self, now: Instant) -> Result<(), &'static str> {
        self.prune(now);
        if self.refused_until.is_some_and(|until| now < until) {
            return Err("the person refused an agent a moment ago; try again later");
        }
        if self.waiting {
            return Err("another agent waits for the person's answer");
        }
        if self.asks.len() >= MAX_ASKS {
            return Err("too many agents asked the person this minute");
        }
        self.asks.push_back(now);
        self.waiting = true;
        Ok(())
    }

    /// The person answered the waiting connection, or it went.
    pub(super) fn answered(&mut self, refused: bool, now: Instant) {
        self.waiting = false;
        if refused {
            self.refused_until = Some(now + REFUSED_FOR);
        }
    }
}

pub(super) fn gate(shared: &Shared) -> MutexGuard<'_, Gate> {
    shared.gate.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One of a bounded number of slots, given back when dropped.
pub(super) struct Slot<'a>(&'a AtomicUsize);

impl<'a> Slot<'a> {
    pub(super) fn take(count: &'a AtomicUsize, max: usize) -> Option<Self> {
        count
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < max).then_some(n + 1)
            })
            .ok()
            .map(|_| Self(count))
    }
}

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// A connection's requests a second: a bucket of [`MAX_RATE`] that fills at that rate.
pub(super) struct Rate {
    tokens: f64,
    at: Instant,
}

impl Rate {
    pub(super) fn new(now: Instant) -> Self {
        Self {
            tokens: MAX_RATE,
            at: now,
        }
    }

    /// Whether one more request may go now.
    pub(super) fn take(&mut self, now: Instant) -> bool {
        let refill = now.duration_since(self.at).as_secs_f64() * MAX_RATE;
        self.tokens = (self.tokens + refill).min(MAX_RATE);
        self.at = now;
        if self.tokens < 1.0 {
            return false;
        }
        self.tokens -= 1.0;
        true
    }
}
