//! Default key bindings, in Emacs notation (ADR 0033, ADR 0034). They
//! follow vanilla Emacs (ADR 0019). Each feature with default keys has a module
//! here, named as in `commands`, with its own table, and [`TABLES`] lists them.
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data. `session_help` words what
//! a session's keys run, for `C-c ?`, as `menus` words its toggles: below both `config` and
//! `dispatch`, which each build it (ADR 0132).

mod agent_sessions;
mod buffers;
mod complete;
mod cursors;
mod dired;
mod edit;
mod files;
mod indent;
mod isearch;
mod jump;
mod kill;
mod lsp;
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
mod review;
mod save;
mod scroll;
mod session;
mod session_help;
mod terminal;
mod theme;
mod vcs;
mod view;
mod windows;
mod worktrees;

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
    dired::BINDINGS,
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
    vcs::BINDINGS,
    view::BINDINGS,
    windows::BINDINGS,
];

pub use menus::built_in_menus;
#[cfg(test)]
pub use session_help::MAX_HELP;
pub use session_help::session_help;

/// The default global keymap, over the commands in `registry`, with the built-in menus' keys.
///
/// # Panics
///
/// If a row of [`TABLES`] does not bind, if `registry` lacks the command that typing runs, or
/// if a row of the menus' keys does not parse or names no built-in menu. The tests build this
/// keymap, so such a mistake cannot ship.
pub fn global_keymap(registry: &Registry) -> Keymap {
    let mut keymap =
        Keymap::from_table(registry, &TABLES.concat()).unwrap_or_else(|e| panic!("{e}"));
    let Some(typing) = registry.lookup(edit::TEXT_COMMAND) else {
        panic!("no command named {}", edit::TEXT_COMMAND);
    };
    keymap.set_text_command(typing);
    // The built-in menus' keys, by the ids `built_in_menus` gives them, which every set built
    // from them keeps (ADR 0117).
    bind_menus(&mut keymap, registry, menus::MENU_KEYS);
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
/// place of it (ADR 0072), of a prompt that completes, with the first one's keys
/// (ADR 0076), and of a review comment's draft, above the global one (ADR 0122). Empty for a
/// registry without their commands, as in tests that bring their own.
pub fn prompt_keymaps(registry: &Registry) -> [Keymap; 4] {
    let keymap = |table: &[(&str, &str)]| Keymap::from_table(registry, table).unwrap_or_default();
    let completing = [minibuffer::MINIBUFFER, complete::COMPLETION].concat();
    [
        keymap(minibuffer::MINIBUFFER),
        keymap(minibuffer::QUESTION),
        keymap(&completing),
        keymap(minibuffer::DRAFT),
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

/// A review buffer's own keymap, above the global one (ADR 0122). Empty for a registry without
/// its commands, as in tests that bring their own.
pub fn review_keymap(registry: &Registry) -> Keymap {
    Keymap::from_table(registry, review::REVIEW).unwrap_or_default()
}

/// A directory listing's own keymap, above the global one in it (ADR 0134). Empty for a
/// registry without its commands, as in tests that bring their own.
pub fn dired_keymap(registry: &Registry) -> Keymap {
    Keymap::from_table(registry, &dired::LISTING.concat()).unwrap_or_default()
}

/// A git status's own keymap, above the global one in it (ADR 0127), with the keys of the
/// built-in menus it opens. Empty for a registry without its commands, as in tests that bring
/// their own.
pub fn git_status_keymap(registry: &Registry) -> Keymap {
    let Ok(mut keymap) = Keymap::from_table(registry, vcs::STATUS) else {
        return Keymap::default();
    };
    bind_menus(&mut keymap, registry, vcs::STATUS_MENUS);
    keymap
}

/// A commit's message's own keymap, above the global one in it (ADR 0127, step 1b). Empty for
/// a registry without its commands.
pub fn git_commit_keymap(registry: &Registry) -> Keymap {
    Keymap::from_table(registry, vcs::MESSAGE).unwrap_or_default()
}

/// The log of git's runs' own keymap, above the global one in it. Empty for a registry without
/// its commands.
pub fn git_log_keymap(registry: &Registry) -> Keymap {
    Keymap::from_table(registry, vcs::LOG).unwrap_or_default()
}

/// The keymap of the list of what discards kept, above the global one in it (ADR 0127, step
/// 1c). Empty for a registry without its commands.
pub fn git_discarded_keymap(registry: &Registry) -> Keymap {
    Keymap::from_table(registry, vcs::DISCARDED).unwrap_or_default()
}

/// `*Spaces*`'s own keymap, above the global one in it (ADR 0121). Empty for a registry
/// without its commands, as in tests that bring their own.
pub fn sessions_keymap(registry: &Registry) -> Keymap {
    Keymap::from_table(registry, agent_sessions::LIST).unwrap_or_default()
}

/// Markdown's own keymap, above the global one in a Markdown buffer (ADR 0115). Empty for a
/// registry without its commands, as in tests that bring their own.
pub fn markdown_keymap(registry: &Registry) -> Keymap {
    Keymap::from_table(registry, markdown::MARKDOWN).unwrap_or_default()
}

/// The keymaps of a `*Brief*` buffer and of a `*Worktrees*` buffer, above the global one there
/// (ADR 0121). Empty for a registry without their commands, as in tests that bring their own.
pub fn worktree_keymaps(registry: &Registry) -> [Keymap; 2] {
    let keymap = |table| Keymap::from_table(registry, table).unwrap_or_default();
    [keymap(worktrees::BRIEF), keymap(worktrees::WORKTREES)]
}

/// The `lsp` minor mode's keymap, above the major mode's in a buffer a language server follows
/// (ADR 0138). Empty for a registry without its commands, as in tests that bring their own.
pub fn lsp_keymap(registry: &Registry) -> Keymap {
    Keymap::from_table(registry, lsp::LSP).unwrap_or_default()
}

/// A terminal's own keymap, above the global one in a terminal's buffer (ADR 0120). Empty for
/// a registry without its commands, as in tests that bring their own.
pub fn terminal_keymap(registry: &Registry) -> Keymap {
    let Ok(mut keymap) = Keymap::from_table(registry, terminal::TERMINAL) else {
        return Keymap::default();
    };
    bind_menus(&mut keymap, registry, terminal::TERMINAL_MENUS);
    keymap
}

/// The keymap of a terminal's buffer that hosts an agent session (ADR 0132): the terminal's own
/// keys and session mode's, which share no key, so one keymap looks up as a minor mode's over
/// the terminal's would. Empty for a registry without their commands, as in tests that bring
/// their own.
pub fn session_keymap(registry: &Registry) -> Keymap {
    let Ok(mut keymap) = Keymap::from_table(
        registry,
        &[terminal::TERMINAL, agent_sessions::SESSION].concat(),
    ) else {
        return Keymap::default();
    };
    bind_menus(&mut keymap, registry, terminal::TERMINAL_MENUS);
    keymap
}

/// Binds each `(keys, menu)` of `table` in `keymap` to the built-in menu of that name, by the id
/// `built_in_menus` gives it.
///
/// # Panics
///
/// If a row does not parse, names no built-in menu or does not bind; the tests build every
/// keymap that calls this, so such a mistake cannot ship.
fn bind_menus(keymap: &mut Keymap, registry: &Registry, table: &[(&str, &str)]) {
    let built_in = built_in_menus(registry);
    for (keys, name) in table {
        let sequence: KeySequence = keys.parse().unwrap_or_else(|e| panic!("{keys}: {e}"));
        let Some(id) = built_in.lookup(name) else {
            panic!("no built-in menu {name} for {keys}");
        };
        keymap
            .bind_menu(&sequence.0, id)
            .unwrap_or_else(|e| panic!("{keys}: {e}"));
    }
}

/// The keys of a server's description at point, while it shows (ADR 0138, Hover). Empty for a
/// registry without its commands, as in tests that bring their own.
pub fn hover_keymap(registry: &Registry) -> Keymap {
    Keymap::from_table(registry, lsp::HOVER).unwrap_or_default()
}

/// The commands a description's layer runs rather than closes for.
pub fn hover_commands(registry: &Registry) -> Vec<oso_core::command::CommandId> {
    lsp::HOVER
        .iter()
        .filter_map(|&(_, name)| registry.lookup(name))
        .collect()
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
        TABLES, agent_sessions, complete, global_keymap, indent, isearch, isearch_commands,
        isearch_keymap, jump, jump_keymap, markdown, markdown_keymap, minibuffer, prefix,
        prefix_keymaps, prompt_keymaps, recovery, recovery_keymaps, review, review_keymap,
        rigid_commands, rigid_keymap, session_keymap, terminal, terminal_keymap, worktree_keymaps,
        worktrees,
    };
    use crate::commands;
    use oso_core::command::Registry;
    use oso_core::key::{KeyPress, KeySequence, Modifiers};
    use oso_core::keymap::{Binding, Keymap};
    use oso_core::menu::HeadAction;

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
    fn the_worktree_keymaps_bind_all_their_rows() {
        let registry = commands::registry();
        let [brief, listing] = worktree_keymaps(&registry);
        for (keymap, table) in [(&brief, worktrees::BRIEF), (&listing, worktrees::WORKTREES)] {
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
    fn the_terminal_keymap_binds_all_its_rows() {
        let registry = commands::registry();
        let keymap = terminal_keymap(&registry);
        for (keys, name) in terminal::TERMINAL {
            let keys: KeySequence = keys.parse().unwrap();
            let Some(Binding::Command(id)) = keymap.lookup(&keys.0) else {
                panic!("{keys} is not bound to a command");
            };
            assert_eq!(registry.get(id).unwrap().name, *name);
        }
        assert_opens_its_menus(&keymap, &registry, terminal::TERMINAL_MENUS);
    }

    #[test]
    fn the_session_keymap_binds_the_terminal_s_rows_and_its_own() {
        let registry = commands::registry();
        let keymap = session_keymap(&registry);
        for (keys, name) in [terminal::TERMINAL, agent_sessions::SESSION].concat() {
            let keys: KeySequence = keys.parse().unwrap();
            let Some(Binding::Command(id)) = keymap.lookup(&keys.0) else {
                panic!("{keys} is not bound to a command");
            };
            assert_eq!(registry.get(id).unwrap().name, name);
        }
        assert_opens_its_menus(&keymap, &registry, terminal::TERMINAL_MENUS);
    }

    /// Each `(keys, menu)` of `table` opens the built-in menu of that name in `keymap`.
    fn assert_opens_its_menus(keymap: &Keymap, registry: &Registry, table: &[(&str, &str)]) {
        let menus = super::built_in_menus(registry);
        for (keys, name) in table {
            let keys: KeySequence = keys.parse().unwrap();
            assert_eq!(
                keymap.lookup(&keys.0),
                Some(Binding::Menu(menus.lookup(name).unwrap())),
                "{keys}"
            );
        }
    }

    /// In a session, `C-c C-<letter>` runs the Agents menu's head of that letter (ADR 0143), so
    /// the two tables cannot drift apart; signs keep their `C-c` key, and the head of that sign
    /// runs the same.
    #[test]
    fn every_session_key_runs_the_agents_menu_s_head_of_its_letter() {
        let registry = commands::registry();
        let menus = super::built_in_menus(&registry);
        let agents = menus.get(menus.lookup("agents").unwrap()).unwrap();
        let head = |key: KeyPress| {
            agents
                .heads()
                .iter()
                .find(|head| head.keys.0 == [key])
                .map(|head| head.action.clone())
        };
        let mut twins = 0;
        for (keys, name) in agent_sessions::SESSION {
            let keys: KeySequence = keys.parse().unwrap();
            let [_, key] = keys.0[..] else {
                panic!("{keys} is not C-c and one key");
            };
            let plain = KeyPress::new(key.modifiers.without(Modifiers::CONTROL), key.key).unwrap();
            let id = registry.lookup(name).unwrap();
            match head(plain) {
                Some(action) => {
                    assert_eq!(action, HeadAction::Command(id), "{keys}");
                    twins += 1;
                }
                // Only session mode's own keys have no head: help, reading, `C-c C-u` and the
                // clipboard's image (ADR 0142); and the pane, `s` in the menu, is `C-c C-p`, as
                // `C-c C-s` is the live search.
                None => assert!(
                    [
                        "session-help",
                        "terminal-allow-agent-read",
                        "terminal-deny-agent-read",
                        "universal-argument",
                        "session-yank-image",
                        "sessions",
                    ]
                    .contains(name),
                    "{keys} {name} has no head in the Agents menu"
                ),
            }
        }
        assert!(twins > 10, "{twins}");
    }

    #[test]
    fn the_review_keymap_binds_all_its_rows() {
        let registry = commands::registry();
        let keymap = review_keymap(&registry);
        for (keys, name) in review::REVIEW {
            let keys: KeySequence = keys.parse().unwrap();
            let Some(Binding::Command(id)) = keymap.lookup(&keys.0) else {
                panic!("{keys} is not bound to a command");
            };
            assert_eq!(registry.get(id).unwrap().name, *name);
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
        let registry = commands::registry();
        let [minibuffer, question, completing, draft] = prompt_keymaps(&registry);
        assert_eq!(minibuffer.len(), minibuffer::MINIBUFFER.len());
        assert_eq!(question.len(), minibuffer::QUESTION.len());
        let both = minibuffer::MINIBUFFER.len() + complete::COMPLETION.len();
        assert_eq!(completing.len(), both);
        for (keys, name) in minibuffer::DRAFT {
            let keys: KeySequence = keys.parse().unwrap();
            let Some(Binding::Command(id)) = draft.lookup(&keys.0) else {
                panic!("{keys} is not bound to a command");
            };
            assert_eq!(registry.get(id).unwrap().name, *name);
        }
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
