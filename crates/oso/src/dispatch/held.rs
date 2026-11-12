//! A key that takes the selected candidate, held while a worker matches the input or builds
//! the list (ADR 0096), or that answers a prompt for a directory before the listing or the look
//! at a link that decides it has come (ADR 0114).
//!
//! Must not depend on: `app` or the worker; the host says when the result has come.
//!
//! `RET` and `TAB` take the selected candidate, which is an older input's while a list past
//! 5000 candidates waits for the worker. `RET` and `M-RET` in a prompt for a directory give a
//! directory only once a listing says it is one, or a look says a link leads to one. Their command is held here instead of run, and
//! runs once the result comes, through the same `execute`, so a directory is gone into and an
//! answer given as it would have been. Any other key drops it, as it comes, before it is
//! resolved: an edit asks for a new result, a move changes the candidate it was to take, a
//! prefix begins another command, and `C-g` drops it and does nothing more, as `C-g` does one
//! thing at a time (ADR 0072). Text typed drops it too.
//!
//! A key from the dev socket is never held: it runs on the rows shown, and a client that wants
//! the newest waits until `get-state` says nothing is pending. A held key replayed later would
//! open what it opens outside the request that sent it.

use oso_core::command::{Args, CommandId, Effects, Invocation};
use oso_core::editor::Editor;
use oso_core::minibuffer::PromptId;

use super::Dispatcher;

/// The commands that take the selected candidate: `RET`'s and `TAB`'s.
const TAKE_THE_SELECTED: [&str; 2] = ["exit-minibuffer", "insert-completion"];

/// The commands that answer a prompt for a directory: `RET`'s and `M-RET`'s (ADR 0114).
const ANSWER_A_DIRECTORY: [&str; 2] = ["exit-minibuffer", "exit-minibuffer-input"];

/// `RET` or `TAB`, held for the result a prompt waits for, or `RET` or `M-RET`, held for what
/// decides a directory's answer: the command, by its id and its name, and the prompt it is for.
pub(super) struct HeldTake {
    id: CommandId,
    name: &'static str,
    args: Args,
    prompt: PromptId,
}

/// Whether the command `name` takes the selected candidate.
pub(super) fn takes_the_selected(name: &str) -> bool {
    TAKE_THE_SELECTED.contains(&name)
}

/// Whether the command `name` waits for what `editor` waits for before it can run: a worker's
/// result for one that takes the selected candidate, or what decides a directory's answer.
pub(super) fn waits(editor: &Editor, name: &str) -> bool {
    (takes_the_selected(name) && editor.completion_pending())
        || (ANSWER_A_DIRECTORY.contains(&name) && editor.directory_answer_waits())
}

impl Dispatcher {
    /// Holds the command `id` if it waits for what the prompt waits for ([`waits`]): a selected
    /// candidate the worker has yet to give, or what decides a directory's answer. `dropped`
    /// says a key was held before this one, which a `C-g` then only drops. Returns whether
    /// nothing more is to be done for the key.
    pub(super) fn hold_for_completion(
        &mut self,
        editor: &Editor,
        id: CommandId,
        args: Args,
        dropped: bool,
    ) -> bool {
        let name: &'static str = self.registry.get(id).map_or("", |command| command.name);
        if dropped && name == "keyboard-quit" {
            return true;
        }
        if self.from_socket || !waits(editor, name) {
            return false;
        }
        let Some(prompt) = editor.prompt().map(oso_core::minibuffer::Prompt::id) else {
            return false;
        };
        self.completion_held = Some(HeldTake {
            id,
            name,
            args,
            prompt,
        });
        true
    }

    /// Drops the key held, as other input came. Returns whether one was.
    pub fn drop_held_completion(&mut self) -> bool {
        self.completion_held.take().is_some()
    }

    /// Runs the key held once the result its prompt waited for has come, if that prompt is
    /// still the innermost and waits for nothing more. Returns what the host should do.
    pub fn run_held_completion(&mut self, editor: &mut Editor) -> Effects {
        let Some(held) = &self.completion_held else {
            return Effects::default();
        };
        if waits(editor, held.name) {
            return Effects::default();
        }
        let Some(held) = self.completion_held.take() else {
            return Effects::default();
        };
        if editor.prompt().map(oso_core::minibuffer::Prompt::id) != Some(held.prompt) {
            return Effects::default();
        }
        self.as_input(editor, |this, editor| {
            let ran = this
                .run_or_read(editor, held.id, held.args, Invocation::default())
                .0;
            ran.union(this.after_prompts(editor))
        })
    }

    /// Whether a key is held for a result.
    pub fn holds_for_completion(&self) -> bool {
        self.completion_held.is_some()
    }
}
