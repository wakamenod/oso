//! Whether the person can see oso's window, and since when (ADR 0182, "Reading" and "Point" 5): in
//! front, as the system said, never assumed, and not hidden.
//!
//! Must not depend on: the app or a window system; the loop says what the system said.

use std::time::Instant;

/// What the system said of oso's window.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(in crate::app::sessions::hosting::native) struct Sight {
    /// Since when it is in front.
    front: Option<Instant>,
    hidden: bool,
}

impl Sight {
    /// It came in front at `now`, or left it.
    pub(super) fn front(&mut self, front: bool, now: Instant) {
        self.front = front.then(|| self.front.unwrap_or(now));
    }

    /// It was hidden, or came in sight at `now`, which is as new to the person as coming in front.
    pub(super) fn hidden(&mut self, hidden: bool, now: Instant) {
        if self.hidden && !hidden {
            self.front = self.front.map(|_| now);
        }
        self.hidden = hidden;
    }

    /// Since when the person can see it, if they can.
    pub(super) fn since(self) -> Option<Instant> {
        self.front.filter(|_| !self.hidden)
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::Sight;

    #[test]
    fn a_window_is_seen_from_when_the_system_said_so() {
        let start = Instant::now();
        let at = |ms| start + Duration::from_millis(ms);
        let mut sight = Sight::default();
        assert_eq!(sight.since(), None, "not told yet");
        sight.front(true, at(0));
        sight.front(true, at(10));
        assert_eq!(sight.since(), Some(at(0)));
        sight.hidden(true, at(20));
        assert_eq!(sight.since(), None);
        sight.hidden(false, at(30));
        assert_eq!(sight.since(), Some(at(30)), "seen again from then");
        sight.hidden(false, at(40));
        assert_eq!(sight.since(), Some(at(30)));
        sight.front(false, at(50));
        sight.hidden(true, at(60));
        sight.hidden(false, at(70));
        assert_eq!(sight.since(), None, "in sight, and not in front");
        sight.front(true, at(80));
        assert_eq!(sight.since(), Some(at(80)));
    }
}
