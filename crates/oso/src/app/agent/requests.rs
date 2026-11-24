//! What the agent channel does not do while a native session's request waits for the person
//! (ADR 0165): its `open` shows no file, with the request in front, and highlighting stays as it
//! is.
//!
//! Must not depend on: how a request is shown or answered; the sessions say whether one waits.

use crate::app::App;
use crate::commands::FONT_LOCK;

/// What an agent is told while a request waits.
const WAITS: &str =
    "a native session's request waits for the person; try again once it is answered";

/// Why `open` waits, if a request waits in front of the person.
fn opens(in_front: bool) -> Option<&'static str> {
    in_front.then_some(WAITS)
}

/// Why command `name` waits, if a request waits on any card.
fn runs(name: &str, any: bool) -> Option<&'static str> {
    (any && name == FONT_LOCK).then_some(WAITS)
}

impl App {
    /// Why the channel shows no file now, if it may not: the person is in the middle of
    /// something, or a request waits in front of them.
    pub(super) fn open_waits(&self) -> Option<&'static str> {
        self.agent_waits()
            .or_else(|| opens(self.request_in_front()))
    }

    /// Why the channel does not run command `name` now, if it may not.
    pub(super) fn run_waits(&self, name: &str) -> Option<&'static str> {
        self.agent_waits()
            .or_else(|| runs(name, self.request_shown()))
    }
}

#[cfg(test)]
mod tests {
    use super::{WAITS, opens, runs};
    use crate::commands::{FONT_LOCK, registry};

    #[test]
    fn a_request_that_waits_holds_an_agents_open_and_its_highlighting() {
        assert_eq!(opens(false), None);
        assert_eq!(opens(true), Some(WAITS));
        assert!(registry().lookup(FONT_LOCK).is_some(), "it is a command");
        assert_eq!(runs(FONT_LOCK, false), None);
        assert_eq!(runs(FONT_LOCK, true), Some(WAITS));
        assert_eq!(runs("forward-char", true), None);
    }
}
