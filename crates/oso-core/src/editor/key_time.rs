//! How long a key's search runs on the UI thread before it leaves the rest to its worker
//! (ADR 0082, ADR 0091, ADR 0113). Its tests are in `key_time_tests`.
//!
//! Must not depend on: `command` or `keymap`; the searches ask this.

use std::time::Instant;

use super::Editor;

impl Editor {
    /// When a key's search that began at `started` leaves the rest to the worker, if it does.
    pub(crate) fn key_until(&self, started: Instant) -> Option<Instant> {
        self.key_time.map(|time| started + time)
    }

    /// Gives keys' searches `time` on the UI thread, or no limit but their work's.
    #[cfg(test)]
    pub(crate) fn set_key_time(&mut self, time: Option<std::time::Duration>) {
        self.key_time = time;
    }
}
