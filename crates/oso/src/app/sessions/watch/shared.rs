//! What the app's other concerns read of the watch: a session's state as the worker last told
//! it, and the spaces, which a worktree session's joins (ADR 0121 step 4).
//!
//! Must not depend on: key presses or how a frame is drawn.

use super::Watch;
use crate::sessions::spaces::Spaces;
use crate::sessions::state::State;
use crate::sessions::worker::SessionKey;

impl Watch {
    /// The state the worker last told of session `key`, if it told one.
    pub(in crate::app) fn state(&self, key: SessionKey) -> Option<State> {
        self.known.get(&key).map(|known| known.state)
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
