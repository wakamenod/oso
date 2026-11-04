//! Default key bindings, in Emacs notation (ADR 0033, ADR 0034). They
//! follow vanilla Emacs (ADR 0019). Each feature in `commands` has a module
//! of the same name here with its own table, and [`TABLES`] lists them.
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

mod motion;
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
    motion::BINDINGS,
    scroll::BINDINGS,
    session::BINDINGS,
];

/// The default global keymap, over the commands in `registry`.
///
/// # Panics
///
/// If a row of [`TABLES`] does not bind. The tests build this keymap, so
/// such a mistake cannot ship.
pub fn global_keymap(registry: &Registry) -> Keymap {
    Keymap::from_table(registry, &TABLES.concat()).unwrap_or_else(|e| panic!("{e}"))
}

#[cfg(test)]
mod tests {
    use super::{TABLES, global_keymap};
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
            assert_eq!(registry.get(*id).unwrap().name, name);
        }
    }

    #[test]
    fn the_order_of_the_tables_does_not_change_what_a_key_does() {
        let registry = commands::registry();
        let reversed: Vec<_> = TABLES
            .iter()
            .rev()
            .flat_map(|table| *table)
            .copied()
            .collect();
        assert_eq!(
            Keymap::from_table(&registry, &reversed).unwrap(),
            global_keymap(&registry)
        );
    }
}
