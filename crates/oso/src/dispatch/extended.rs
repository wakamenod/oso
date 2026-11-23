//! What `M-x` completes over: the registry's commands, each with its binding and the first
//! line of its docstring, and the label that says the prefix argument it passes on
//! (ADR 0076).
//!
//! Must not depend on: `app` or winit; key sequences stay testable without a window.
//!
//! A command that reads the key that ran it is left out, and so is one bound only in a
//! keymap that is not active beneath the prompt, such as a question's `answer-yes`, as
//! Emacs's `command-completion-default-include-p` does. The binding shown is the shortest
//! in the layers active now that no higher layer shadows, and that no shorter part of it cuts
//! short, as `where-is` finds it. A global binding the user's keys shadow binds nothing, so a
//! command the user left without a key is listed with none (ADR 0108, ADR 0110).
//!
//! [`Dispatcher::where_is`] names the key a message tells the person to press, the same way, and
//! through the menus those layers open too (ADR 0143), which `M-x` lists as well.

use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::sync::Arc;

use oso_core::command::{ArgSpec, CommandId, Prefix};
use oso_core::complete::Candidate;
use oso_core::editor::Editor;
use oso_core::key::KeyPress;
use oso_core::keymap::{Binding, Layers, display_keys, is_cancel};
use oso_core::menu::{HeadAction, MenuId};

use super::Dispatcher;

impl Dispatcher {
    /// The commands `M-x` lists, opened over what `editor` shows now.
    pub(super) fn command_candidates(&self, editor: &Editor) -> Arc<[Candidate]> {
        let reading = self.keymaps.reading(editor);
        let layers = self.keymaps.active(&reading, None, editor);
        // The shortest binding of each command, by keys and then by how it reads.
        let mut shortest: HashMap<CommandId, (usize, String)> = HashMap::new();
        for keymap in layers.iter() {
            keymap.bindings(|keys, id| {
                if reaches(&layers, keys, Binding::Command(id)) {
                    keep_shortest(&mut shortest, id, keys);
                }
            });
        }
        // A command a menu's head runs is listed with that key, as one a key binds (ADR 0143).
        for (keys, id) in self.menu_heads(&layers) {
            keep_shortest(&mut shortest, id, &keys);
        }
        let mut bound = HashSet::new();
        let over_global = Layers {
            user: self.keymaps.user(),
            ..Layers::global(&self.keymaps.global)
        };
        for keymap in self.keymaps.all() {
            let global = std::ptr::eq(keymap, &self.keymaps.global);
            keymap.bindings(|keys, id| {
                if !global || reaches(&over_global, keys, Binding::Command(id)) {
                    bound.insert(id);
                }
            });
        }
        self.registry
            .iter()
            .filter(|(id, command)| {
                command.args != ArgSpec::Key && (shortest.contains_key(id) || !bound.contains(id))
            })
            .filter(|(_, command)| self.lists(command.name))
            .map(|(id, command)| {
                let key = shortest.get(&id).map_or("", |(_, text)| text.as_str());
                Candidate::new(command.name, key, command.summary())
            })
            .collect()
    }

    /// The menus `open-menu` lists: every menu in force, each with the shortest key that opens
    /// it in the layers active now and its title, as `M-x` shows a command's (ADR 0117).
    pub(super) fn menu_candidates(&self, editor: &Editor) -> Arc<[Candidate]> {
        let reading = self.keymaps.reading(editor);
        let layers = self.keymaps.active(&reading, None, editor);
        let mut shortest: HashMap<MenuId, (usize, String)> = HashMap::new();
        for keymap in layers.iter() {
            keymap.menu_bindings(|keys, id| {
                if reaches(&layers, keys, Binding::Menu(id)) {
                    keep_shortest(&mut shortest, id, keys);
                }
            });
        }
        self.menus_in_force()
            .iter()
            .map(|(id, definition)| {
                let key = shortest.get(&id).map_or("", |(_, text)| text.as_str());
                Candidate::new(definition.name(), key, definition.title())
            })
            .collect()
    }

    /// The keys that run the command named `name` from the current buffer, as a message names
    /// them (ADR 0143): the shortest of its bindings in the layers active now, and of a menu's
    /// key there and the keys of its head that runs the command, through up to
    /// [`MAX_MENU_DEPTH`] menus (`C-c a S u`); else `M-x` and the name. A rebinding in
    /// `config.toml` shows, and the heads a menu leaves out or dims still count, as they do once
    /// their reason goes.
    pub fn where_is(&self, editor: &Editor, name: &str) -> String {
        let reading = self.keymaps.reading(editor);
        self.where_is_in(&self.keymaps.active(&reading, None, editor), name)
    }

    /// [`Dispatcher::where_is`] in the layers every buffer has, the user's keys over the global
    /// ones, for a hint that holds wherever the person is, as the pane's modeline is (ADR 0143).
    pub fn where_is_anywhere(&self, name: &str) -> String {
        let layers = Layers {
            user: self.keymaps.user(),
            ..Layers::global(&self.keymaps.global)
        };
        self.where_is_in(&layers, name)
    }

    /// [`Dispatcher::where_is`] in the layers a native session's composer has, for the
    /// placeholder it shows whichever window is selected (ADR 0170).
    pub fn where_is_in_composer(&self, name: &str) -> String {
        let layers = Layers {
            major: Some(self.keymaps.composer()),
            user: self.keymaps.user(),
            ..Layers::global(&self.keymaps.global)
        };
        self.where_is_in(&layers, name)
    }

    /// The shortest keys of the command named `name` in `layers`, through the menus they open,
    /// else `M-x` and the name.
    fn where_is_in(&self, layers: &Layers<'_>, name: &str) -> String {
        let fallback = || format!("M-x {name}");
        let Some(target) = self.registry.lookup(name) else {
            return fallback();
        };
        let mut shortest = HashMap::new();
        for keymap in layers.iter() {
            keymap.bindings(|keys, id| {
                if id == target && reaches(layers, keys, Binding::Command(id)) {
                    keep_shortest(&mut shortest, (), keys);
                }
            });
        }
        for (keys, id) in self.menu_heads(layers) {
            if id == target {
                keep_shortest(&mut shortest, (), &keys);
            }
        }
        shortest.remove(&()).map_or_else(fallback, |(_, text)| text)
    }

    /// The keys of each command a menu's head runs, from the menus a key in `layers` opens and
    /// those their heads open, through up to [`MAX_MENU_DEPTH`] menus: the menu's key, then the
    /// head's. Heads a menu leaves out or dims count too, as they do once their reason goes.
    fn menu_heads(&self, layers: &Layers<'_>) -> Vec<(Vec<KeyPress>, CommandId)> {
        let mut menus: Vec<(Vec<KeyPress>, MenuId)> = Vec::new();
        for keymap in layers.iter() {
            keymap.menu_bindings(|keys, id| {
                if reaches(layers, keys, Binding::Menu(id)) {
                    menus.push((keys.to_vec(), id));
                }
            });
        }
        let in_force = self.menus_in_force();
        let (mut seen, mut heads) = (HashSet::new(), Vec::new());
        for _ in 0..MAX_MENU_DEPTH {
            // A menu is opened from its shortest key, the first of the order `keep_shortest`
            // keeps, so its heads are named through that key.
            menus.sort_by_cached_key(|(keys, _)| {
                let text = display_keys(keys).to_string();
                (keys.len(), text.len(), text)
            });
            let mut next = Vec::new();
            for (keys, id) in menus {
                let Some(definition) = in_force.get(id).filter(|_| seen.insert(id)) else {
                    continue;
                };
                for head in definition.heads() {
                    let keys = [&keys[..], &head.keys.0[..]].concat();
                    match head.action {
                        HeadAction::Command(id) => heads.push((keys, id)),
                        HeadAction::Menu(id) => next.push((keys, id)),
                        HeadAction::Switch(_) | HeadAction::Quit => {}
                    }
                }
            }
            menus = next;
        }
        heads
    }

    /// Whether `M-x` lists the command named `name`, of those it could: in a test that gave
    /// only some, whether it is one of them.
    #[cfg(test)]
    fn lists(&self, name: &str) -> bool {
        self.m_x_only.is_none_or(|names| names.contains(&name))
    }

    #[cfg(not(test))]
    fn lists(&self, _: &str) -> bool {
        true
    }
}

/// Keeps `keys` for `id` in `shortest` if they are the shortest yet: the fewest keys, then the
/// fewest characters spelled out, then the first in order. Keys longer than the best are not
/// spelled out at all.
fn keep_shortest<K: Hash + Eq>(
    shortest: &mut HashMap<K, (usize, String)>,
    id: K,
    keys: &[KeyPress],
) {
    if shortest.get(&id).is_some_and(|(len, _)| keys.len() > *len) {
        return;
    }
    let text = display_keys(keys).to_string();
    let better = shortest
        .get(&id)
        .is_none_or(|(len, other)| (keys.len(), text.len(), &text) < (*len, other.len(), other));
    if better {
        shortest.insert(id, (keys.len(), text));
    }
}

/// How many menus deep [`Dispatcher::where_is`] looks: a menu's key, a head that opens another,
/// and its heads, as the Agents menu opens the space and review menus.
const MAX_MENU_DEPTH: usize = 3;

/// Whether `keys` come to `bound`, a command or a menu, in `layers`, as the resolver reads them
/// a key at a time: no shorter part of them is bound to a command or a menu or left undefined,
/// and `C-g` comes only first, where the user's keys do not bind it (ADR 0108).
fn reaches(layers: &Layers<'_>, keys: &[KeyPress], bound: Binding<'_>) -> bool {
    let cancels = |key: &KeyPress| is_cancel(*key);
    let Some((first, rest)) = keys.split_first() else {
        return false;
    };
    if rest.iter().any(cancels) {
        return false;
    }
    let layers = &Layers {
        user: layers.user.filter(|_| !cancels(first)),
        ..*layers
    };
    layers.lookup(keys) == Some(bound)
        && (1..keys.len()).all(|len| {
            !matches!(
                layers.lookup(&keys[..len]),
                Some(Binding::Command(_) | Binding::Menu(_) | Binding::Undefined)
            )
        })
}

/// `prompt` with the prefix argument it passes on before it, as Emacs's `M-x` says it:
/// `C-u M-x`, `16 M-x`, `- M-x`, `3 M-x`.
pub(super) fn with_prefix(prompt: &str, prefix: Option<Prefix>) -> String {
    match prefix {
        None => prompt.to_owned(),
        Some(Prefix::Universal(4)) => format!("C-u {prompt}"),
        Some(Prefix::Negative) => format!("- {prompt}"),
        Some(prefix) => format!("{} {prompt}", prefix.value()),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::with_prefix;
    use crate::dispatch::Dispatcher;
    use oso_core::buffer::Buffer;
    use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Prefix, Reach, Registry};
    use oso_core::complete::Candidate;
    use oso_core::editor::Editor;
    use oso_core::keymap::Keymap;
    use oso_core::keymap::Layers;

    /// A message names a command's keys as bound, through the menus a key opens too
    /// (ADR 0143).
    #[test]
    fn where_is_names_the_shortest_keys_through_menus_or_m_x() {
        let dispatcher = Dispatcher::new();
        let editor = Editor::with_buffer(Buffer::from_text("test", "text"));
        let where_is = |name| dispatcher.where_is(&editor, name);
        assert_eq!(where_is("save-buffer"), "C-x C-s");
        assert_eq!(where_is("session-explain"), "C-c a e");
        assert_eq!(where_is("session-worktree"), "C-c a w");
        // Through the space menu the Agents menu opens.
        assert_eq!(where_is("space-use-repo"), "C-c a S u");
        assert_eq!(where_is("no-such-command"), "M-x no-such-command");
        assert_eq!(dispatcher.where_is_anywhere("sessions"), "C-c a s");
        // A longer key of the user's to the same menu does not win.
        let mut dispatcher = Dispatcher::new();
        let registry = crate::commands::registry();
        let agents = crate::bindings::built_in_menus(&registry)
            .lookup("agents")
            .unwrap();
        let mut user = Keymap::new();
        let keys: oso_core::key::KeySequence = "C-c x y".parse().unwrap();
        user.bind_menu(&keys.0, agents).unwrap();
        dispatcher.set_user_keymap(user);
        assert_eq!(dispatcher.where_is(&editor, "session-explain"), "C-c a e");
        let (session, editor) = crate::dispatch::terminal_tests::in_session();
        assert_eq!(session.where_is(&editor, "session-explain"), "C-c C-e");
        assert_eq!(session.where_is(&editor, "session-send"), "C-c a m");
    }

    #[test]
    fn m_x_lists_commands_with_their_shortest_binding_and_summary() {
        let dispatcher = Dispatcher::new();
        let editor = Editor::with_buffer(Buffer::from_text("test", "text"));
        let items = dispatcher.command_candidates(&editor);
        let find = |name: &str| items.iter().find(|item| item.text() == name);
        let save = find("save-buffer").unwrap();
        assert_eq!(save.binding(), "C-x C-s");
        assert!(save.annotation().starts_with("Save"));
        assert_eq!(find("forward-char").map(Candidate::binding), Some("C-f"));
        assert_eq!(find("yank").map(Candidate::binding), Some("C-y"));
        assert!(find("execute-extended-command").is_some());
        // Not one that reads its key, nor one of a keymap not active.
        for name in [
            "self-insert-command",
            "digit-argument",
            "answer-yes",
            "exit-minibuffer",
        ] {
            assert!(find(name).is_none(), "{name}");
        }
    }

    #[test]
    fn m_x_lists_once_each_command_bound_globally_or_nowhere_that_reads_no_key() {
        let dispatcher = Dispatcher::new();
        let editor = Editor::with_buffer(Buffer::from_text("test", "text"));
        let items = dispatcher.command_candidates(&editor);
        let listed: HashSet<&str> = items.iter().map(Candidate::text).collect();
        assert_eq!(listed.len(), items.len(), "a command is listed twice");
        // Beneath a prompt over the text, only the global keymap is active, so a command
        // bound in another keymap alone is left out.
        let (mut global, mut elsewhere) = (HashSet::new(), HashSet::new());
        for keymap in dispatcher.keymaps.all() {
            let into = if std::ptr::eq(keymap, &dispatcher.keymaps.global) {
                &mut global
            } else {
                &mut elsewhere
            };
            keymap.bindings(|_, id| {
                into.insert(id);
            });
        }
        // A command a global menu key's menu runs counts as bound globally (ADR 0143).
        let layers = Layers::global(&dispatcher.keymaps.global);
        global.extend(dispatcher.menu_heads(&layers).into_iter().map(|(_, id)| id));
        for (id, command) in dispatcher.registry.iter() {
            let runs =
                command.args != ArgSpec::Key && (global.contains(&id) || !elsewhere.contains(&id));
            assert_eq!(listed.contains(command.name), runs, "{}", command.name);
        }
    }

    #[test]
    fn a_command_bound_nowhere_is_listed_with_no_key() {
        fn noop(_: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
            Ok(())
        }
        let mut registry = Registry::new();
        for name in ["bound", "unbound"] {
            let command = Command {
                name,
                doc: "A test command.",
                args: ArgSpec::None,
                reach: Reach::Look,
                run: noop,
                state: None,
            };
            registry.register(command).unwrap();
        }
        let global = Keymap::from_table(&registry, &[("C-c b", "bound")]).unwrap();
        let dispatcher = Dispatcher::with(registry, global);
        let editor = Editor::with_buffer(Buffer::from_text("test", "text"));
        let items = dispatcher.command_candidates(&editor);
        let listed: Vec<_> = items
            .iter()
            .map(|item| (item.text(), item.binding()))
            .collect();
        assert_eq!(listed, [("bound", "C-c b"), ("unbound", "")]);
    }

    #[test]
    fn the_label_says_the_prefix_argument() {
        assert_eq!(with_prefix("M-x", None), "M-x");
        assert_eq!(with_prefix("M-x", Some(Prefix::Universal(4))), "C-u M-x");
        assert_eq!(with_prefix("M-x", Some(Prefix::Universal(16))), "16 M-x");
        assert_eq!(with_prefix("M-x", Some(Prefix::Negative)), "- M-x");
        assert_eq!(with_prefix("M-x", Some(Prefix::Number(3))), "3 M-x");
    }
}
