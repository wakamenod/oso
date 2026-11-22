//! The user's keymap from the configuration and their `[keys.session]`, which wait for a
//! pending prefix key to go in, and the keymaps they replace, for the caller to drop elsewhere
//! (ADR 0108, ADR 0132).
//!
//! Must not depend on: `app` or winit.

use oso_core::keymap::Keymap;

use crate::terminal::route;

use super::super::Dispatcher;

/// The user's keymap from the configuration, which waits for a pending prefix key to go in
/// (ADR 0108).
impl Dispatcher {
    /// Puts in the user's keymap that waited for a prefix key, once none is pending. The one
    /// it replaces waits in turn, for [`Dispatcher::take_replaced_keymap`].
    pub(in crate::dispatch) fn take_waiting_user(&mut self) {
        if !self.resolver.pending().is_empty() {
            return;
        }
        self.menus.take_waiting();
        if let Some(user) = self.user_waiting.take() {
            let old = std::mem::replace(&mut self.keymaps.user, user);
            self.replaced.push(old);
        }
        if let Some((keys, help)) = self.session_waiting.take() {
            let old = self.put_session_keys(keys, help);
            self.replaced.push(old);
        }
    }

    /// Puts the user's keys from the configuration just above the global keymap, in place of
    /// those before; an empty keymap takes them away (ADR 0108). While a prefix key is
    /// pending, they wait for it to resolve, so that its next key reads the keymaps it
    /// started in.
    ///
    /// Returns the keymap this replaces now, the one in force or one that was waiting, for the
    /// caller to drop elsewhere: a large one takes milliseconds to drop. One that a waiting
    /// keymap replaces later is kept for [`Dispatcher::take_replaced_keymap`].
    pub fn set_user_keymap(&mut self, user: Keymap) -> Option<Keymap> {
        if self.resolver.pending().is_empty() {
            if let Some(stale) = self.user_waiting.take() {
                self.replaced.push(stale);
            }
            Some(std::mem::replace(&mut self.keymaps.user, user))
        } else {
            self.user_waiting.replace(user)
        }
    }

    /// Puts the user's `[keys.session]` above a session's terminal's keys, in place of those
    /// before, with `help`, what `C-c ?` says of them, or `None` for the default line
    /// (ADR 0132). They wait for a pending prefix key as [`Dispatcher::set_user_keymap`]'s do,
    /// and the keymap this replaces now is returned in the same way.
    ///
    /// Keys that would take `C-c C-c` or `C-c C-k` from the terminal are not put in, as the
    /// configuration refuses them: `C-c` would no longer reach the program, or char mode
    /// would not switch. The default line is said then.
    pub fn set_session_keys(&mut self, keys: Keymap, help: Option<String>) -> Option<Keymap> {
        let takes = |key| keys.lookup(&[route::C_C, key]).is_some();
        let (keys, help) = if takes(route::C_C) || takes(route::C_K) {
            log::warn!("session keys that bind C-c C-c or C-c C-k were not put in");
            (Keymap::new(), None)
        } else {
            (keys, help)
        };
        if self.resolver.pending().is_empty() {
            if let Some((stale, _)) = self.session_waiting.take() {
                self.replaced.push(stale);
            }
            Some(self.put_session_keys(keys, help))
        } else {
            self.session_waiting
                .replace((keys, help))
                .map(|(stale, _)| stale)
        }
    }

    /// Puts in the user's session keys and their line, and returns the keys they replace.
    fn put_session_keys(&mut self, keys: Keymap, help: Option<String>) -> Keymap {
        self.keymaps.session_help.set(help);
        std::mem::replace(&mut self.keymaps.user_session, keys)
    }

    /// A user's keymap that a waiting one replaced, once its prefix key resolved, for the
    /// caller to drop elsewhere. Call it until it gives `None`.
    pub fn take_replaced_keymap(&mut self) -> Option<Keymap> {
        self.replaced.pop()
    }
}
