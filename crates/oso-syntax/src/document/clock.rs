//! The worker's time, which the limits of a parse and of highlighting count (ADR 0172): the
//! thread's CPU time from the clock the tip crate passes, never more than the wall clock's.
//!
//! Must not depend on: buffers, windows or faces; it sees two clocks, and runs only on the
//! worker.
//!
//! - A [`Stopwatch`] times one interval, capped at its wall time: a piece is judged by it.
//! - [`Spent`] adds intervals up for a limit, capped in all, so that a coarse clock's error
//!   (Windows counts in 15.6 ms ticks) stays within a tick and does not add up.
//! - A slice ends on the wall clock, not here.

use std::time::{Duration, Instant};

/// The worker thread's CPU time: `oso_platform::thread_time::now` in the tip crate. `None`
/// times that interval on the wall clock instead. The public signatures that take it write
/// the type out, as the API snapshot shows it.
pub(crate) type Clock = fn() -> Option<Duration>;

/// A clock that reads none, so that tests time on the wall clock, as the worker did before
/// ADR 0172: wall time is never less than CPU time, so a crafted text fails no later.
#[cfg(test)]
pub(crate) fn wall() -> Option<Duration> {
    None
}

/// Times an interval in the worker's time: the CPU time its clock reads, but never more than
/// the wall clock's, so that a coarse clock (Windows counts in 15.6 ms ticks) judges a piece no
/// stricter than the wall clock did; the wall clock's alone where the clock reads none
/// (ADR 0172). [`Spent`] adds intervals up for a limit.
pub(in crate::document) struct Stopwatch {
    clock: Clock,
    wall: Instant,
    cpu: Option<Duration>,
}

impl Stopwatch {
    /// Starts now: one read of each clock.
    pub(in crate::document) fn start(clock: Clock) -> Self {
        Self {
            clock,
            wall: Instant::now(),
            cpu: clock(),
        }
    }

    /// When it started, on the wall clock, which slices end by.
    pub(in crate::document) fn started(&self) -> Instant {
        self.wall
    }

    /// The worker's time since it started: one read of each clock.
    pub(in crate::document) fn elapsed(&self) -> Duration {
        let (cpu, wall) = self.read();
        cpu.min(wall)
    }

    /// The CPU time since it started, or the wall clock's where the clock reads none, and the
    /// wall clock's time: one read of each clock.
    fn read(&self) -> (Duration, Duration) {
        let wall = self.wall.elapsed();
        match (self.cpu, (self.clock)()) {
            (Some(before), Some(after)) => (after.saturating_sub(before), wall),
            _ => (wall, wall),
        }
    }
}

/// The worker's time over many intervals, as a limit counts it: the CPU time in all, but never
/// more than the wall clock's in all. Capped in all, not at each interval, so that a coarse
/// clock's error stays within one of its ticks and does not add up (ADR 0172).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::document) struct Spent {
    cpu: Duration,
    wall: Duration,
}

impl Spent {
    /// Adds the interval since `watch` started: one read of each clock.
    pub(in crate::document) fn add(&mut self, watch: &Stopwatch) {
        let (cpu, wall) = watch.read();
        self.cpu = self.cpu.saturating_add(cpu);
        self.wall = self.wall.saturating_add(wall);
    }

    /// The worker's time in all.
    pub(in crate::document) fn total(self) -> Duration {
        self.cpu.min(self.wall)
    }
}
