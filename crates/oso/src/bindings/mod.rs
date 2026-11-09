//! Default key bindings, in Emacs notation (ADR 0033, ADR 0034). They
//! follow vanilla Emacs (ADR 0019). Each feature with default keys has a module
//! here, named as in `commands`, with its own table, and [`TABLES`] lists them.
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

mod buffers;
mod complete;
mod cursors;
mod edit;
mod files;
mod isearch;
mod kill;
mod mark;
mod messages;
mod minibuffer;
mod motion;
mod prefix;
mod recovery;
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
    complete::BINDINGS,
    cursors::BINDINGS,
    edit::BINDINGS,
    files::BINDINGS,
    isearch::BINDINGS,
    kill::BINDINGS,
    mark::BINDINGS,
    buffers::BINDINGS,
    messages::BINDINGS,
    #[cfg(feature = "dev")]
    minibuffer::DEV,
    motion::BINDINGS,
    prefix::BINDINGS,
    recovery::BINDINGS,
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

/// The keymaps of a prompt that reads an answer, above the global one, of a question, in
/// place of it (ADR 0072), and of a prompt that completes, with the first one's keys
/// (ADR 0076). Empty for a registry without their commands, as in tests that bring their
/// own.
pub fn prompt_keymaps(registry: &Registry) -> [Keymap; 3] {
    let keymap = |table: &[(&str, &str)]| Keymap::from_table(registry, table).unwrap_or_default();
    let completing = [minibuffer::MINIBUFFER, complete::COMPLETION].concat();
    [
        keymap(minibuffer::MINIBUFFER),
        keymap(minibuffer::QUESTION),
        keymap(&completing),
    ]
}

/// The keymap of a search under way, in place of the others (ADR 0082), with its text
/// command. Empty for a registry without its commands, as in tests that bring their own.
pub fn isearch_keymap(registry: &Registry) -> Keymap {
    let mut keymap = Keymap::from_table(registry, isearch::ISEARCH).unwrap_or_default();
    if let Some(typing) = registry.lookup(isearch::TEXT_COMMAND) {
        keymap.set_text_command(typing);
    }
    keymap
}

/// The commands that run in a search rather than end it: those of its keymap, its text
/// command, and the global ones that start a search, which repeat it once it is on.
#[cfg(any(test, feature = "dev"))]
pub fn isearch_commands(registry: &Registry) -> Vec<oso_core::command::CommandId> {
    let names = isearch::ISEARCH
        .iter()
        .chain(isearch::BINDINGS)
        .map(|&(_, name)| name);
    names
        .chain([isearch::TEXT_COMMAND])
        .filter_map(|name| registry.lookup(name))
        .collect()
}

/// The keymaps of the list of emergency copies and of a buffer showing one, above the global
/// one (ADR 0064). Empty for a registry without their commands, as in tests that bring their
/// own.
pub fn recovery_keymaps(registry: &Registry) -> (Keymap, Keymap) {
    let keymap = |table| Keymap::from_table(registry, table).unwrap_or_default();
    (keymap(recovery::LIST), keymap(recovery::COPY))
}

#[cfg(test)]
mod tests {
    use super::{
        TABLES, complete, global_keymap, isearch, isearch_commands, isearch_keymap, minibuffer,
        prefix, prefix_keymaps, prompt_keymaps, recovery, recovery_keymaps,
    };
    use crate::commands;
    use oso_core::key::KeySequence;
    use oso_core::keymap::{Binding, Keymap};

    #[test]
    fn the_keys_of_the_emergency_copies_resolve_to_their_commands() {
        let registry = commands::registry();
        let (list, copy) = recovery_keymaps(&registry);
        for (keymap, table) in [(&list, recovery::LIST), (&copy, recovery::COPY)] {
            for (keys, name) in table {
                let keys: KeySequence = keys.parse().unwrap();
                let Some(Binding::Command(id)) = keymap.lookup(&keys.0) else {
                    panic!("{keys} is not bound to a command");
                };
                assert_eq!(registry.get(id).unwrap().name, *name);
            }
        }
    }

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

    #[test]
    fn the_prompt_keymaps_bind_all_their_rows() {
        // A clash would leave a keymap empty, and `RET` in `M-x` to the global keymap.
        let [minibuffer, question, completing] = prompt_keymaps(&commands::registry());
        assert_eq!(minibuffer.len(), minibuffer::MINIBUFFER.len());
        assert_eq!(question.len(), minibuffer::QUESTION.len());
        let both = minibuffer::MINIBUFFER.len() + complete::COMPLETION.len();
        assert_eq!(completing.len(), both);
    }

    #[test]
    fn the_isearch_keymap_binds_all_its_rows_and_types() {
        let registry = commands::registry();
        let keymap = isearch_keymap(&registry);
        assert_eq!(keymap.len(), isearch::ISEARCH.len());
        assert_eq!(
            keymap.text_command(),
            registry.lookup(isearch::TEXT_COMMAND)
        );
        let wanted = isearch::ISEARCH.len() + isearch::BINDINGS.len() + 1;
        assert_eq!(isearch_commands(&registry).len(), wanted);
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
