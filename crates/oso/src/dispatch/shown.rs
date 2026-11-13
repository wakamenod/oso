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

/// Writes `prefix` as the echo area shows it, `C-u 5`, after what `out` holds; nothing for none.
pub(super) fn prefix_text(prefix: Option<Prefix>, out: &mut String) {
    match prefix {
        None => {}
        Some(Prefix::Universal(4)) => out.push_str("C-u"),
        Some(Prefix::Negative) => out.push_str("C-u -"),
        Some(prefix) => {
            let _ = write!(out, "C-u {}", prefix.value());
        }
    }
}

impl Dispatcher {
    /// Tells the echo area the keys in progress: the prefix argument and the keys of a
    /// pending prefix (ADR 0058, ADR 0063). In an open menu, only the keys that begin a head
    /// or a pending prefix: its title row shows the prefix argument (ADR 0117).
    pub(super) fn show_keys_in_progress(&self, editor: &mut Editor) {
        let mut keys = String::new();
        let pending = match self.menus.pending() {
            Some(in_menu) => {
                let pending = self.resolver.pending();
                if pending.is_empty() { in_menu } else { pending }
            }
            None => {
                prefix_text(self.prefix, &mut keys);
                self.resolver.pending()
            }
        };
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
