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

    /// The name of the space session `key` is in, as `*Sessions*` shows it.
    pub(in crate::app) fn space_name(&self, key: SessionKey) -> Option<&str> {
        self.placed.get(&key).map(String::as_str)
    }

    /// The spaces.
    pub(in crate::app) fn spaces(&self) -> &Spaces {
        &self.spaces
    }

    /// The spaces, to add a worktree's or close one.
    pub(in crate::app) fn spaces_mut(&mut self) -> &mut Spaces {
        &mut self.spaces
    }
}
