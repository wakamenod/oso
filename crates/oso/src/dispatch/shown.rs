//! How the keys in progress are shown: in the echo area, with the prefix argument, and in the
//! log, where a key that types a character is masked (ADR 0028, ADR 0058, ADR 0063).
//!
//! Must not depend on: `app` or winit; this only formats keys.

use std::fmt::Write;

use oso_core::command::Prefix;
use oso_core::editor::Editor;
use oso_core::key::KeyPress;
use oso_core::keymap::display_keys;
use oso_core::minibuffer::Kind;

use super::Dispatcher;

/// Keys in Emacs notation, each key that types a character as `<char>` where `mask` holds:
/// always in the log, as typed text stays out of it (ADR 0028), and on screen in a passphrase
/// prompt, where the user's keys can make a typing key part of a prefix (ADR 0110).
pub(super) fn masked(keys: &[KeyPress], mask: bool) -> String {
    if !mask {
        return display_keys(keys).to_string();
    }
    let mut text = String::new();
    for (i, key) in keys.iter().enumerate() {
        if i > 0 {
            text.push(' ');
        }
        if key.text().is_some() {
            text.push_str("<char>");
        } else {
            let _ = write!(text, "{key}");
        }
    }
    text
}

/// Whether the innermost prompt reads a passphrase, so typing keys stay off the screen.
pub(super) fn reads_passphrase(editor: &Editor) -> bool {
    editor
        .prompt()
        .is_some_and(|prompt| prompt.kind() == Kind::Passphrase)
}

impl Dispatcher {
    /// Tells the echo area the keys in progress: the prefix argument and the keys of a
    /// pending prefix (ADR 0058, ADR 0063).
    pub(super) fn show_keys_in_progress(&self, editor: &mut Editor) {
        let pending = self.resolver.pending();
        let mut keys = String::new();
        match self.prefix {
            None => {}
            Some(Prefix::Universal(4)) => keys.push_str("C-u"),
            Some(Prefix::Negative) => keys.push_str("C-u -"),
            Some(prefix) => {
                let _ = write!(keys, "C-u {}", prefix.value());
            }
        }
        if !pending.is_empty() {
            if !keys.is_empty() {
                keys.push(' ');
            }
            keys.push_str(&masked(pending, reads_passphrase(editor)));
        }
        if !keys.is_empty() {
            keys.push('-');
        }
        editor.set_keys_in_progress(&keys);
    }
}
