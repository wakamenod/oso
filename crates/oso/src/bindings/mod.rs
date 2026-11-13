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
mod indent;
mod isearch;
mod jump;
mod kill;
mod macros;
mod mark;
mod markdown;
mod menus;
mod messages;
mod minibuffer;
mod motion;
mod prefix;
mod projects;
mod recovery;
mod replace;
mod save;
mod scroll;
mod session;
mod theme;
mod view;
mod windows;

use oso_core::command::Registry;
use oso_core::key::KeySequence;
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
    indent::BINDINGS,
    isearch::BINDINGS,
    jump::BINDINGS,
    kill::BINDINGS,
    macros::BINDINGS,
    mark::BINDINGS,
    buffers::BINDINGS,
    messages::BINDINGS,
    #[cfg(feature = "dev")]
    minibuffer::DEV,
    motion::BINDINGS,
    prefix::BINDINGS,
    projects::BINDINGS,
    recovery::BINDINGS,
    replace::BINDINGS,
    save::BINDINGS,
    scroll::BINDINGS,
    session::BINDINGS,
    theme::BINDINGS,
    view::BINDINGS,
    windows::BINDINGS,
];

/// The default global keymap, over the commands in `registry`.
///
/// # Panics
///
/// If a row of [`TABLES`] does not bind, or `registry` lacks the command that
pub use menus::menus;

/// typing runs. The tests build this keymap, so such a mistake cannot ship.
pub fn global_keymap(registry: &Registry) -> Keymap {
    let mut keymap =
        Keymap::from_table(registry, &TABLES.concat()).unwrap_or_else(|e| panic!("{e}"));
    let Some(typing) = registry.lookup(edit::TEXT_COMMAND) else {
        panic!("no command named {}", edit::TEXT_COMMAND);
    };
    keymap.set_text_command(typing);
    // The built-in menus' keys, by the ids `menus` gives them, which every set built from them
    // keeps (ADR 0117).
    let built_in = menus(registry);
    for (keys, name) in menus::MENU_KEYS {
        let (Ok(sequence), Some(id)) = (keys.parse::<KeySequence>(), built_in.lookup(name)) else {
            panic!("no built-in menu {name} for {keys}");
        };
        keymap
            .bind_menu(&sequence.0, id)
            .unwrap_or_else(|e| panic!("{keys}: {e}"));
    }
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

/// The keymap of a jump under way, in place of the others (ADR 0116), with its text command.
/// Empty for a registry without its commands, as in tests that bring their own.
pub fn jump_keymap(registry: &Registry) -> Keymap {
    let mut keymap = Keymap::from_table(registry, jump::JUMP).unwrap_or_default();
    if let Some(typing) = registry.lookup(jump::TEXT_COMMAND) {
        keymap.set_text_command(typing);
    }
    keymap
}

/// The keymap of a query-replace under way, in place of the others (ADR 0113). Empty for a
/// registry without its commands, as in tests that bring their own.
pub fn replace_keymap(registry: &Registry) -> Keymap {
    Keymap::from_table(registry, replace::REPLACE).unwrap_or_default()
}

/// The commands that run in a query-replace rather than end it: those of its keymap.
#[cfg(any(test, feature = "dev"))]
pub fn replace_commands(registry: &Registry) -> Vec<oso_core::command::CommandId> {
    replace::REPLACE
        .iter()
        .filter_map(|&(_, name)| registry.lookup(name))
        .collect()
}

/// The commands that run in a jump rather than end it: those of its keymap and its text
/// command (ADR 0116).
#[cfg(any(test, feature = "dev"))]
pub fn jump_commands(registry: &Registry) -> Vec<oso_core::command::CommandId> {
    let names = jump::JUMP.iter().map(|&(_, name)| name);
    names
        .chain([jump::TEXT_COMMAND])
        .filter_map(|name| registry.lookup(name))
        .collect()
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

/// The keymap of the layer `C-x TAB` opens (ADR 0115). Empty for a registry without its
/// commands, as in tests that bring their own.
pub fn rigid_keymap(registry: &Registry) -> Keymap {
    Keymap::from_table(registry, indent::RIGID).unwrap_or_default()
}

/// Markdown's own keymap, above the global one in a Markdown buffer (ADR 0115). Empty for a
/// registry without its commands, as in tests that bring their own.
pub fn markdown_keymap(registry: &Registry) -> Keymap {
    Keymap::from_table(registry, markdown::MARKDOWN).unwrap_or_default()
}

/// The commands the layer of `C-x TAB` runs rather than closes for.
pub fn rigid_commands(registry: &Registry) -> Vec<oso_core::command::CommandId> {
    indent::RIGID
        .iter()
        .filter_map(|&(_, name)| registry.lookup(name))
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
        TABLES, complete, global_keymap, indent, isearch, isearch_commands, isearch_keymap, jump,
        jump_keymap, markdown, markdown_keymap, minibuffer, prefix, prefix_keymaps, prompt_keymaps,
        recovery, recovery_keymaps, rigid_commands, rigid_keymap,
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
    fn the_indent_keymaps_bind_all_their_rows() {
        let registry = commands::registry();
        assert_eq!(rigid_keymap(&registry).len(), indent::RIGID.len());
        assert_eq!(markdown_keymap(&registry).len(), markdown::MARKDOWN.len());
        assert_eq!(rigid_commands(&registry).len(), indent::RIGID.len());
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

    #[test]
    fn the_jump_keymap_binds_all_its_rows_and_types() {
        let registry = commands::registry();
        let keymap = jump_keymap(&registry);
        assert_eq!(keymap.len(), jump::JUMP.len());
        assert_eq!(keymap.text_command(), registry.lookup(jump::TEXT_COMMAND));
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
