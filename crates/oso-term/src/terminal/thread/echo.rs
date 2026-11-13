//! Watching for a password prompt (ADR 0120, Q17): when the thread looks at the PTY's flags,
//! and what the next snapshot says of them. A look costs one system call, so the thread looks
//! before it writes input, after output, for a short while after either, and while echo is
//! off, and never while the terminal is idle with echo on.
//!
//! Must not depend on: the PTY or the engine; the thread looks and reports here.

use std::time::{Duration, Instant};

/// After input or output, the looks that follow it, from the activity: a program turns echo
/// off around the prompt it prints, often just after it.
const TAIL: [Duration; 2] = [Duration::from_millis(20), Duration::from_millis(250)];
/// While echo is off, how often the thread looks for it to come back on.
const WHILE_OFF: Duration = Duration::from_millis(250);

/// What the thread knows of the PTY's flags.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::terminal) struct EchoWatch {
    /// What the last look saw.
    off: bool,
    /// Whether any look since the last snapshot saw a password prompt.
    seen: bool,
    /// What the last snapshot said.
    said: bool,
    /// When input or output last came, for the tail.
    active: Option<Instant>,
    /// When the thread last looked.
    looked: Option<Instant>,
}

impl EchoWatch {
    /// A look at `now` saw a password prompt (`off`) or not.
    pub(in crate::terminal) fn saw(&mut self, off: bool, now: Instant) {
        self.off = off;
        self.seen |= off;
        self.looked = Some(now);
    }

    /// Input was written or output read at `now`: the tail of looks starts again.
    pub(in crate::terminal) fn activity(&mut self, now: Instant) {
        self.active = Some(now);
    }

    /// When the thread looks next with nothing else to wake it, if it does.
    pub(in crate::terminal) fn due(&self) -> Option<Instant> {
        let looked = self.looked?;
        let tail = self.active.and_then(|active| {
            TAIL.iter()
                .map(|after| active + *after)
                .find(|at| *at > looked)
        });
        let off = self.off.then(|| looked + WHILE_OFF);
        tail.into_iter().chain(off).min()
    }

    /// What the next snapshot says: off now, or off at a look since the last one said it was
    /// on, so a password typed and entered between two snapshots is still seen once.
    pub(in crate::terminal) fn echo_off(&self) -> bool {
        self.off || (self.seen && !self.said)
    }

    /// Whether the next snapshot says something the last did not.
    pub(in crate::terminal) fn changed(&self) -> bool {
        self.echo_off() != self.said
    }

    /// A snapshot went, saying `echo_off()`.
    pub(in crate::terminal) fn published(&mut self) {
        self.said = self.echo_off();
        self.seen = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_prompt_seen_between_two_snapshots_is_said_once() {
        let mut watch = EchoWatch::default();
        assert!(!watch.echo_off() && !watch.changed());
        watch.saw(true, Instant::now());
        watch.saw(false, Instant::now());
        assert!(watch.echo_off() && watch.changed());
        watch.published();
        // Said once; the next snapshot says it is back on.
        assert!(!watch.echo_off() && watch.changed());
        watch.published();
        assert!(!watch.changed());
    }

    #[test]
    fn echo_off_stays_said_while_it_lasts() {
        let mut watch = EchoWatch::default();
        watch.saw(true, Instant::now());
        watch.published();
        watch.saw(true, Instant::now());
        assert!(watch.echo_off() && !watch.changed());
    }

    #[test]
    fn echo_back_on_is_said_after_a_prompt_was() {
        let mut watch = EchoWatch::default();
        watch.saw(true, Instant::now());
        watch.published();
        watch.saw(true, Instant::now());
        watch.saw(false, Instant::now());
        assert!(!watch.echo_off() && watch.changed());
    }

    #[test]
    fn an_idle_terminal_with_echo_on_is_not_looked_at() {
        let now = Instant::now();
        let mut watch = EchoWatch::default();
        assert_eq!(watch.due(), None);
        watch.activity(now);
        watch.saw(false, now);
        assert_eq!(watch.due(), Some(now + TAIL[0]));
        watch.saw(false, now + TAIL[0]);
        assert_eq!(watch.due(), Some(now + TAIL[1]));
        watch.saw(false, now + TAIL[1]);
        assert_eq!(watch.due(), None);
    }

    #[test]
    fn echo_off_is_looked_at_until_it_comes_back() {
        let now = Instant::now();
        let mut watch = EchoWatch::default();
        watch.saw(true, now);
        assert_eq!(watch.due(), Some(now + WHILE_OFF));
        watch.saw(false, now + WHILE_OFF);
        assert_eq!(watch.due(), None);
    }
}
