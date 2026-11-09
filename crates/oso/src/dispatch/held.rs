//! A key that takes the selected candidate, held while a worker matches the input or builds
//! the list (ADR 0096).
//!
//! Must not depend on: `app` or the worker; the host says when the result has come.
//!
//! `RET` and `TAB` take the selected candidate, which is an older input's while a list past
//! 5000 candidates waits for the worker. Their command is held here instead of run, and runs
//! once the result comes, through the same `execute`, so a directory is gone into and an
//! answer given as it would have been. Any other input drops it: an edit asks for a new
//! result, a move changes the candidate it was to take, and `C-g` drops it and does nothing
//! more, as `C-g` does one thing at a time (ADR 0072).

use oso_core::command::{Args, CommandId, Effects, Invocation};
use oso_core::editor::Editor;
use oso_core::minibuffer::PromptId;

use super::Dispatcher;

/// The commands that take the selected candidate: `RET`'s and `TAB`'s.
const TAKE_THE_SELECTED: [&str; 2] = ["exit-minibuffer", "insert-completion"];

/// A command held for the result a prompt waits for.
pub(super) struct Held {
    id: CommandId,
    args: Args,
    prompt: PromptId,
    from_socket: bool,
}

impl Dispatcher {
    /// Holds the command `id` if it takes a selected candidate the worker has yet to give,
    /// and drops the one held for any other key. Returns whether nothing more is to be done
    /// for the key.
    pub(super) fn hold_for_completion(
        &mut self,
        editor: &Editor,
        id: CommandId,
        args: Args,
    ) -> bool {
        let name = self.registry.get(id).map(|command| command.name);
        if self.completion_held.take().is_some() && name == Some("keyboard-quit") {
            return true;
        }
        let takes = name.is_some_and(|name| TAKE_THE_SELECTED.contains(&name));
        let Some(prompt) = editor.prompt().map(oso_core::minibuffer::Prompt::id) else {
            return false;
        };
        if !takes || !editor.completion_pending() {
            return false;
        }
        self.completion_held = Some(Held {
            id,
            args,
            prompt,
            from_socket: self.from_socket,
        });
        true
    }

    /// Drops the command held, as text came that changes the input.
    pub(super) fn drop_held_completion(&mut self) {
        self.completion_held = None;
    }

    /// Runs the command held once the result its prompt waited for has come, if that prompt
    /// is still the innermost and waits for nothing more. Returns what the host should do.
    pub fn run_held_completion(&mut self, editor: &mut Editor) -> Effects {
        if editor.completion_pending() {
            return Effects::default();
        }
        let Some(held) = self.completion_held.take() else {
            return Effects::default();
        };
        if editor.prompt().map(oso_core::minibuffer::Prompt::id) != Some(held.prompt) {
            return Effects::default();
        }
        let before = self.from_socket;
        self.from_socket = held.from_socket;
        let effects = self.as_input(editor, |this, editor| {
            let ran = this
                .run_or_read(editor, held.id, held.args, Invocation::default())
                .0;
            ran.union(this.after_prompts(editor))
        });
        self.from_socket = before;
        effects
    }

    /// Whether a command is held for a result.
    #[cfg(test)]
    pub fn holds_for_completion(&self) -> bool {
        self.completion_held.is_some()
    }
}
