//! `C-y` and `M-y` in isearch (ADR 0082, ADR 0091): the newest kill added to the string,
//! and right after it, the next older one in its place.
//!
//! Must not depend on: `command` or `keymap`.

use std::time::Instant;

use super::kill_text;
use crate::editor::{Editor, YankPopError};

impl Editor {
    /// `C-y`: adds the newest kill, its pieces joined by line breaks. A kill from a buffer
    /// that may hold secrets makes the searched buffer one, as a yank does (ADR 0074), so that
    /// where the search goes says no more of the kill than its text would.
    pub fn isearch_yank_kill(&mut self) {
        self.isearch_add_kill(0, None);
    }

    /// `M-y` right after `C-y` or `M-y`: takes back what it added, as `DEL` does, and adds the
    /// next older kill in its place, round to the newest past the oldest, passing secret ones,
    /// so that none reaches the string by being passed over (ADR 0091).
    ///
    /// # Errors
    ///
    /// [`YankPopError::NotAfterYank`] if the step before was no yank, and
    /// [`YankPopError::NoOther`] with no other kill to add.
    pub fn isearch_yank_pop(&mut self) -> Result<(), YankPopError> {
        let state = self.isearch.as_ref().ok_or(YankPopError::NotAfterYank)?;
        let (id, steps) = state.yanked.ok_or(YankPopError::NotAfterYank)?;
        if steps != state.steps.len() {
            return Err(YankPopError::NotAfterYank);
        }
        let n = self
            .next_older(id, |entry| !entry.is_secret())
            .ok_or(YankPopError::NoOther)?;
        // The key's time counts from here, so taking back what it added counts toward it.
        let started = Instant::now();
        self.isearch_delete();
        self.isearch_add_kill(n, Some(started));
        Ok(())
    }

    /// Adds the `n`th newest kill, as `C-y` adds the newest, and notes it for `M-y`, the key's
    /// time to search counted from `started` if given.
    fn isearch_add_kill(&mut self, n: usize, started: Option<Instant>) {
        let Some(state) = &self.isearch else {
            return;
        };
        let room = state.room();
        let Some(entry) = self.kills.ring.get(n) else {
            return;
        };
        let id = entry.id();
        let (text, secret) = (kill_text(entry, room), entry.is_secret());
        if secret {
            let id = state.buffer;
            if let Some(state) = &mut self.isearch {
                state.around_secret = true;
            }
            if self
                .buffer_mut(id)
                .is_some_and(|buffer| buffer.treat_as_secret())
            {
                self.note_secret_shown();
                self.message("Buffer now treated as secret");
            }
        }
        let before = self.isearch.as_ref().map(|state| state.steps.len());
        self.isearch_add_taken(&text, started);
        // Only a step it added can be taken back for the next.
        if let Some(state) = &mut self.isearch {
            let added = Some(state.steps.len()) > before;
            state.yanked = added.then_some((id, state.steps.len()));
        }
    }
}
