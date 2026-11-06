//! Default key bindings, in Emacs notation (ADR 0033, ADR 0034). They
//! follow vanilla Emacs (ADR 0019). Each feature with default keys has a module
//! here, named as in `commands`, with its own table, and [`TABLES`] lists them.
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

mod edit;
mod motion;
mod prefix;
mod save;
mod scroll;
mod session;

use oso_core::command::Registry;
use oso_core::keymap::Keymap;

/// Every feature's bindings. A new feature adds a module and one line here, in
/// alphabetical order and never joined with another, so that features added at
/// the same time rarely touch the same line. The order does not change what a
/// key does: a key bound twice fails to build the keymap, whichever comes first.
#[rustfmt::skip]
const TABLES: &[&[(&str, &str)]] = &[
    edit::BINDINGS,
    motion::BINDINGS,
    prefix::BINDINGS,
    save::BINDINGS,
    scroll::BINDINGS,
    session::BINDINGS,
];

/// The default global keymap, over the commands in `registry`.
///
/// # Panics
///
/// If a row of [`TABLES`] does not bind, or `registry` lacks the command that
/// typing runs. The tests build this keymap, so such a mistake cannot ship.
pub fn global_keymap(registry: &Registry) -> Keymap {
    let mut keymap =
        Keymap::from_table(registry, &TABLES.concat()).unwrap_or_else(|e| panic!("{e}"));
    let Some(typing) = registry.lookup(edit::TEXT_COMMAND) else {
        panic!("no command named {}", edit::TEXT_COMMAND);
    };
    keymap.set_text_command(typing);
    keymap
}

/// The keymaps active while a prefix argument reads digits and a minus sign, above
/// the global one (ADR 0058). Empty for a registry without their commands, as in
/// tests that bring their own: such a prefix never starts.
pub fn prefix_keymaps(registry: &Registry) -> (Keymap, Keymap) {
    let keymap = |table| Keymap::from_table(registry, table).unwrap_or_default();
    (keymap(prefix::DIGITS), keymap(prefix::MINUS))
}

#[cfg(test)]
mod tests {
    use super::{TABLES, global_keymap, prefix, prefix_keymaps};
    use crate::commands;
    use oso_core::key::KeySequence;
    use oso_core::keymap::{Binding, Keymap};

    #[test]
    fn every_default_binding_resolves_to_its_command() {
        let registry = commands::registry();
        let keymap = global_keymap(&registry);
        for (keys, name) in TABLES.concat() {
            let keys: KeySequence = keys.parse().unwrap();
            let Some(Binding::Command(id)) = keymap.lookup(&keys.0) else {
                panic!("{keys} is not bound to a command");
            };
            assert_eq!(registry.get(id).unwrap().name, name);
        }
    }

    #[test]
    fn the_prefix_keymaps_bind_all_their_rows() {
        let (digits, minus) = prefix_keymaps(&commands::registry());
        assert_eq!(digits.len(), prefix::DIGITS.len());
        assert_eq!(minus.len(), prefix::MINUS.len());
    }

    /// The benchmarks time a key that runs nothing, and `big_trip_max_ms` means
    /// something else if this one comes to run a command.
    #[test]
    fn the_key_the_benchmarks_send_stays_unbound() {
        let registry = commands::registry();
        let keys: KeySequence = "<f35>".parse().unwrap();
        assert_eq!(global_keymap(&registry).lookup(&keys.0), None);
    }

    /// Why the order of the tables cannot change what a key does.
    #[test]
    fn rows_that_conflict_fail_in_either_order() {
        let registry = commands::registry();
        for (first, second) in [
            (("C-f", "forward-char"), ("C-f", "backward-char")),
            (("C-x", "forward-char"), ("C-x C-c", "keyboard-quit")),
        ] {
            for rows in [[first, second], [second, first]] {
                assert!(Keymap::from_table(&registry, &rows).is_err(), "{rows:?}");
            }
        }
    }
}
