//! Default key bindings, in Emacs notation (ADR 0033, ADR 0034). They
//! follow vanilla Emacs (ADR 0019).
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

use oso_core::command::Registry;
use oso_core::keymap::Keymap;

/// The default global keymap: key sequences and the commands they run.
pub const GLOBAL: &[(&str, &str)] = &[
    ("C-f", "forward-char"),
    ("<right>", "forward-char"),
    ("C-b", "backward-char"),
    ("<left>", "backward-char"),
    ("C-n", "next-line"),
    ("<down>", "next-line"),
    ("C-p", "previous-line"),
    ("<up>", "previous-line"),
    ("C-a", "move-beginning-of-line"),
    ("<home>", "move-beginning-of-line"),
    ("C-e", "move-end-of-line"),
    ("<end>", "move-end-of-line"),
    ("M-<", "beginning-of-buffer"),
    ("M->", "end-of-buffer"),
    ("C-v", "scroll-up-command"),
    ("<next>", "scroll-up-command"),
    ("M-v", "scroll-down-command"),
    ("<prior>", "scroll-down-command"),
    ("C-g", "keyboard-quit"),
    ("C-x C-c", "save-buffers-kill-terminal"),
];

/// The default global keymap, over the commands in `registry`.
///
/// # Panics
///
/// If a row of [`GLOBAL`] does not bind. The tests build this keymap, so
/// such a mistake cannot ship.
pub fn global_keymap(registry: &Registry) -> Keymap {
    Keymap::from_table(registry, GLOBAL).unwrap_or_else(|e| panic!("{e}"))
}

#[cfg(test)]
mod tests {
    use super::{GLOBAL, global_keymap};
    use crate::commands;
    use oso_core::key::KeySequence;
    use oso_core::keymap::Binding;

    #[test]
    fn every_default_binding_resolves_to_its_command() {
        let registry = commands::registry();
        let keymap = global_keymap(&registry);
        for (keys, name) in GLOBAL {
            let keys: KeySequence = keys.parse().unwrap();
            let Some(Binding::Command(id)) = keymap.lookup(&keys.0) else {
                panic!("{keys} is not bound to a command");
            };
            assert_eq!(registry.get(*id).unwrap().name, *name);
        }
    }
}
