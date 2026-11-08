//! What `M-x` completes over: the registry's commands, each with its binding and the first
//! line of its docstring, and the label that says the prefix argument it passes on
//! (ADR 0076).
//!
//! Must not depend on: `app` or winit; key sequences stay testable without a window.
//!
//! A command that reads the key that ran it is left out, and so is one bound only in a
//! keymap that is not active beneath the prompt, such as a question's `answer-yes`, as
//! Emacs's `command-completion-default-include-p` does. The binding shown is the shortest
//! in the layers active now that no higher layer shadows, as `where-is` finds it.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use oso_core::command::{ArgSpec, CommandId, Prefix};
use oso_core::complete::Candidate;
use oso_core::editor::Editor;
use oso_core::keymap::{Binding, display_keys};

use super::{Dispatcher, Keymaps};

impl Dispatcher {
    /// The commands `M-x` lists, opened over what `editor` shows now.
    pub(super) fn command_candidates(&self, editor: &Editor) -> Arc<[Candidate]> {
        let reading = self.keymaps.reading();
        let layers = self.keymaps.active(&reading, None, editor);
        // The shortest binding of each command, by keys and then by how it reads.
        let mut shortest: HashMap<CommandId, (usize, String)> = HashMap::new();
        for keymap in layers.iter() {
            keymap.bindings(|keys, id| {
                if layers.lookup(keys) != Some(Binding::Command(id)) {
                    return;
                }
                let text = display_keys(keys).to_string();
                let rank = |len: usize, text: &str| (len, text.len(), text.to_owned());
                let better = shortest
                    .get(&id)
                    .is_none_or(|(len, other)| rank(keys.len(), &text) < rank(*len, other));
                if better {
                    shortest.insert(id, (keys.len(), text));
                }
            });
        }
        let mut bound = HashSet::new();
        for keymap in self.keymaps.all() {
            keymap.bindings(|_, id| {
                bound.insert(id);
            });
        }
        self.registry
            .iter()
            .filter(|(id, command)| {
                command.args != ArgSpec::Key && (shortest.contains_key(id) || !bound.contains(id))
            })
            .map(|(id, command)| {
                let key = shortest.get(&id).map_or("", |(_, text)| text.as_str());
                Candidate::new(command.name, key, first_line(command.doc))
            })
            .collect()
    }
}

impl Keymaps {
    /// Every keymap, active or not.
    fn all(&self) -> impl Iterator<Item = &oso_core::keymap::Keymap> {
        [&self.global]
            .into_iter()
            .chain(&self.prefix)
            .chain(&self.copy)
            .chain(&self.prompt)
    }
}

/// The first line of a docstring, its summary.
fn first_line(doc: &str) -> &str {
    doc.lines().next().unwrap_or_default()
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
    use super::{first_line, with_prefix};
    use crate::dispatch::Dispatcher;
    use oso_core::buffer::Buffer;
    use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Prefix, Registry};
    use oso_core::complete::Candidate;
    use oso_core::editor::Editor;
    use oso_core::keymap::Keymap;

    #[test]
    fn m_x_lists_commands_with_their_shortest_binding_and_summary() {
        let dispatcher = Dispatcher::new();
        let editor = Editor::with_buffer(Buffer::from_text("test", "text"));
        let items = dispatcher.command_candidates(&editor);
        let find = |name: &str| items.iter().find(|item| item.text() == name);
        let save = find("save-buffer").unwrap();
        assert_eq!(save.key(), "C-x C-s");
        assert!(save.annotation().starts_with("Save"));
        assert_eq!(find("forward-char").map(Candidate::key), Some("C-f"));
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
                run: noop,
            };
            registry.register(command).unwrap();
        }
        let global = Keymap::from_table(&registry, &[("C-c b", "bound")]).unwrap();
        let dispatcher = Dispatcher::with(registry, global);
        let editor = Editor::with_buffer(Buffer::from_text("test", "text"));
        let items = dispatcher.command_candidates(&editor);
        let listed: Vec<_> = items.iter().map(|item| (item.text(), item.key())).collect();
        assert_eq!(listed, [("bound", "C-c b"), ("unbound", "")]);
    }

    #[test]
    fn the_label_says_the_prefix_argument() {
        assert_eq!(with_prefix("M-x", None), "M-x");
        assert_eq!(with_prefix("M-x", Some(Prefix::Universal(4))), "C-u M-x");
        assert_eq!(with_prefix("M-x", Some(Prefix::Universal(16))), "16 M-x");
        assert_eq!(with_prefix("M-x", Some(Prefix::Negative)), "- M-x");
        assert_eq!(with_prefix("M-x", Some(Prefix::Number(3))), "3 M-x");
        assert_eq!(first_line("Summary.\n\nMore."), "Summary.");
    }
}
