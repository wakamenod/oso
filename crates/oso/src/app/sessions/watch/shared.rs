//! What the app's other concerns read of the watch: a session's state as the worker last told
//! it and since when, its space's name, and the spaces, which a worktree session's joins (ADR
//! 0121 step 4). The agents' session tools read the same (step 6).
//!
//! Must not depend on: key presses or how a frame is drawn.

use std::time::Instant;

use super::Watch;
use crate::sessions::spaces::Spaces;
use crate::sessions::state::State;
use crate::sessions::worker::SessionKey;

impl Watch {
    /// The state the worker last told of session `key`, if it told one.
    pub(in crate::app) fn state(&self, key: SessionKey) -> Option<State> {
        self.known.get(&key).map(|known| known.state)
    }

    /// Since when session `key` is in the state the worker last told of it.
    pub(in crate::app) fn since(&self, key: SessionKey) -> Option<Instant> {
        self.known.get(&key).map(|known| known.since)
    }

    /// The slug of the space session `key` is in, as agents name it (ADR 0159).
    pub(in crate::app) fn space_slug(&self, key: SessionKey) -> Option<&str> {
        self.placed.get(&key).map(String::as_str)
    }

    /// What the selected window's modeline calls the space in front, cleaned as the list's
    /// fields are (ADR 0159); empty with none.
    pub(in crate::app) fn front_label(&self) -> String {
        (self.spaces.current())
            .and_then(|at| self.spaces.get(at))
            .map_or_else(String::new, |space| {
                crate::sessions::recap::clean(space.label())
            })
    }

    /// The spaces.
    pub(in crate::app) fn spaces(&self) -> &Spaces {
        &self.spaces
    }

    /// The spaces, to hold a worktree in one or forget it.
    pub(in crate::app) fn spaces_mut(&mut self) -> &mut Spaces {
        &mut self.spaces
    }
}
