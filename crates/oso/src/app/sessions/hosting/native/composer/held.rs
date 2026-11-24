//! A native composer's send that waits for a dictation's fix (ADR 0175, step 2, the author's
//! choice): `C-c C-c` or `C-u C-c C-c` while the modeline says `fix` waits for the fixes
//! running then to end, and then sends what the composer holds, the fixed text.
//!
//! Must not depend on: key presses or how a frame is drawn; the dictation says which fixes run
//! and when they end.
//!
//! - One wait a session: a second send there waits in its place.
//! - The send goes only with the text the person sent, as the fix left it: the wait keeps the
//!   composer and its revision, which only the fix's own edit moves on. A composer edited,
//!   replaced or gone meanwhile drops the wait, and says so.
//! - A send now goes as a send: the turn it meant to interrupt may have ended meanwhile.
//! - `C-c C-k` drops the wait, and does nothing more; so does `dictation-cancel`.
//! - The send then goes as any does: the composer's checks and the queue's rules apply then.
//! - An exit or a restart drops every wait, and one that waits for the sessions waits for these
//!   too.

use oso_core::buffer::BufferId;

use super::Composing;
use crate::app::App;
use crate::sessions::worker::SessionKey;

/// A send that waits for a dictation's fixes.
#[derive(Debug)]
pub(in crate::app::sessions::hosting::native) struct Held {
    /// The fixes it waits for: those running when it was asked.
    fixes: Vec<u64>,
    /// The composer it was asked in.
    buffer: BufferId,
    /// That composer's revision then, or after a fix's edit there.
    revision: u64,
}

impl Composing {
    /// Whether a send waits here for a dictation's fix.
    pub(in crate::app::sessions::hosting::native) fn holds_a_send(&self) -> bool {
        self.held.is_some()
    }

    /// Whether a send that waits, or the edit of a queued message, is under way here.
    pub(in crate::app::sessions::hosting::native) fn under_way(&self) -> bool {
        self.held.is_some() || self.queue.editing().is_some()
    }
}

impl App {
    /// Holds native session `key`'s send while the modeline says `fix`. Whether it was held.
    pub(super) fn hold_send_for_fix(&mut self, key: SessionKey) -> bool {
        let Some(fixes) = self.dictation_fixes_shown() else {
            return false;
        };
        let Some(native) = self.hosting.natives.map.get(&key) else {
            return false;
        };
        let Some(buffer) = native.composing.buffer else {
            return false;
        };
        let Some(revision) = self.editor.buffer(buffer).map(|b| b.revision()) else {
            return false;
        };
        if let Some(native) = self.hosting.natives.map.get_mut(&key) {
            native.composing.held = Some(Held {
                fixes,
                buffer,
                revision,
            });
        }
        self.editor
            .message("It goes once the dictation's fix ends; C-c C-k drops it");
        true
    }

    /// Drops native session `key`'s waiting send, if there is one, and says so. Whether there
    /// was.
    pub(super) fn drop_held_send(&mut self, key: SessionKey) -> bool {
        let dropped = (self.hosting.natives.map.get_mut(&key))
            .and_then(|native| native.composing.held.take())
            .is_some();
        if dropped {
            self.editor
                .message("The send that waited for the fix is dropped");
        }
        dropped
    }

    /// The waiting sends whose composer still holds what the person sent, unchanged: a fix's
    /// edit about to land moves theirs on with it, through [`App::held_sends_follow`].
    pub(in crate::app) fn held_sends_unchanged(&self) -> Vec<SessionKey> {
        (self.hosting.natives.map.iter())
            .filter(|(_, native)| {
                native.composing.held.as_ref().is_some_and(|held| {
                    let now = self.editor.buffer(held.buffer).map(|b| b.revision());
                    now == Some(held.revision)
                })
            })
            .map(|(key, _)| *key)
            .collect()
    }

    /// The fix's own edit landed: the sends in `keys`, unchanged before it, take their
    /// composer's revision now.
    pub(in crate::app) fn held_sends_follow(&mut self, keys: &[SessionKey]) {
        for key in keys {
            let Some(native) = self.hosting.natives.map.get_mut(key) else {
                continue;
            };
            if let Some(held) = &mut native.composing.held
                && let Some(buffer) = self.editor.buffer(held.buffer)
            {
                held.revision = buffer.revision();
            }
        }
    }

    /// The sends whose fixes have all ended go, unless the composer changed meanwhile, or oso
    /// is on its way out: then each is dropped, and says so.
    pub(in crate::app) fn send_held_composers(&mut self) {
        let leaving = self.leaving.is_under_way();
        let due: Vec<(SessionKey, bool)> = (self.hosting.natives.map.iter())
            .filter_map(|(key, native)| {
                let held = native.composing.held.as_ref()?;
                let ended = !held.fixes.iter().any(|&id| self.dictation_fix_runs(id));
                let same = native.composing.buffer == Some(held.buffer)
                    && self.editor.buffer(held.buffer).map(|b| b.revision()) == Some(held.revision);
                (ended || leaving).then_some((*key, same && !leaving))
            })
            .collect();
        for (key, goes) in due {
            if let Some(native) = self.hosting.natives.map.get_mut(&key) {
                native.composing.held = None;
            }
            match goes {
                true => self.composer_send(key, false),
                false => self.editor.message(
                    "The send that waited for the fix is dropped: the message changed meanwhile",
                ),
            }
        }
    }

    /// As oso exits, a restart starts or `dictation-cancel` stops the fixes: no waiting send
    /// goes. Says so if one waited.
    pub(in crate::app) fn drop_held_sends(&mut self) {
        let mut dropped = false;
        for native in self.hosting.natives.map.values_mut() {
            dropped |= native.composing.held.take().is_some();
        }
        if dropped {
            self.editor
                .message("The send that waited for the fix is dropped: C-c C-c sends it");
        }
    }

    /// Whether any composer's send waits for a fix, for an exit that waits for the sessions.
    pub(in crate::app) fn sends_held(&self) -> bool {
        (self.hosting.natives.map.values()).any(|native| native.composing.held.is_some())
    }
}
