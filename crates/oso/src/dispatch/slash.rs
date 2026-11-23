//! A native session's slash commands as the person types (ADR 0165, step 2d, the author's Q1
//! (a)): `/` typed as an empty composer's first character opens `native-composer-command`'s
//! prompt, as `C-c /` does; `C-g` there leaves the `/` typed. Not an agent's `/`, nor one the
//! host runs.
//! - `/model` or `/effort` chosen there opens its argument's prompt, `Model:` or `Effort:`, and
//!   so does `C-c /` on one the composer begins with (part 3c), when the host listed any.
//!
//! Must not depend on: `app` or winit; the host lists the commands the prompt completes over.

use oso_core::buffer::Mode;
use oso_core::command::{Args, CommandId, Effects, Invocation, Prefix};
use oso_core::editor::Editor;

use super::Dispatcher;
use crate::commands::agent_sessions::{SLASH_LIST, composer_argument};

/// The command whose prompt lists the slash commands.
const SLASH_COMMAND: &str = "native-composer-command";

impl Dispatcher {
    /// After command `id` ran: the person's `/` that left the composer holding only it opens
    /// the slash commands' prompt.
    pub(super) fn complete_slash(&mut self, editor: &mut Editor, id: CommandId) -> Effects {
        // A macro's `/` opens it too, so that its next keys go to the prompt, as a menu's do.
        if !self.persons_typing(editor, id) || editor.prompt().is_some() {
            return Effects::default();
        }
        let window = editor.window();
        let alone = window.selections.len() == 1
            && editor.buffer(window.buffer()).is_some_and(|buffer| {
                let text = buffer.text();
                buffer.mode() == Mode::Composer
                    && text.len() == 1
                    && text.chunks().next() == Some("/")
            });
        // With no command listed yet, a `/` is only a `/`.
        if !alone || editor.listed_candidates(SLASH_LIST).is_empty() {
            return Effects::default();
        }
        let Some(command) = self.registry.lookup(SLASH_COMMAND) else {
            return Effects::default();
        };
        self.run_or_read(editor, command, Args::NONE, Invocation::default())
            .0
    }

    /// After command `id` ran: `/model` or `/effort` chosen in the slash commands' prompt
    /// opens its argument's prompt (part 3c), with a prefix argument, so that the answer takes
    /// no word the composer held after it as the argument.
    pub(super) fn complete_argument(&mut self, editor: &mut Editor, id: CommandId) -> Effects {
        if !self.is_slash_command(id) || editor.prompt().is_some() {
            return Effects::default();
        }
        let Some(argument) = self.argument_of(editor) else {
            return Effects::default();
        };
        let args = Args::with_prefix(Some(Prefix::universal(None)));
        (self.run_or_read(editor, argument, args, Invocation::default())).0
    }

    /// The command to run in place of `id`: the slash commands' on a `/model` or `/effort` the
    /// selected composer begins with completes its argument instead (part 3c).
    pub(super) fn in_place_of(&self, editor: &Editor, id: CommandId) -> CommandId {
        let instead = self.is_slash_command(id).then(|| self.argument_of(editor));
        instead.flatten().unwrap_or(id)
    }

    /// Whether `id` is the slash commands' prompt's command.
    fn is_slash_command(&self, id: CommandId) -> bool {
        self.registry.get(id).map(|command| command.name) == Some(SLASH_COMMAND)
    }

    /// The command that completes the argument of the `/model` or `/effort` the selected
    /// composer begins with, if the host listed any for it.
    fn argument_of(&self, editor: &Editor) -> Option<CommandId> {
        let argument = composer_argument(editor)?;
        if editor.listed_candidates(argument.list).is_empty() {
            return None;
        }
        self.registry.lookup(argument.completes)
    }
}
