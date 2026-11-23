//! A native session's slash commands as the person types (ADR 0165, step 2d, the author's Q1
//! (a)): `/` typed as an empty composer's first character opens `native-composer-command`'s
//! prompt, as `C-c /` does; `C-g` there leaves the `/` typed. Not an agent's `/`, nor one the
//! host runs.
//! - `/model` or `/effort` chosen there opens its argument's prompt, `Model:` or `Effort:`, and
//!   so does `C-c /` on one the composer begins with (part 3c), when the host listed any.
//!
//! Must not depend on: `app` or winit; the host lists the commands the prompt completes over.

use oso_core::buffer::Mode;
use oso_core::command::{Args, CommandId, Effects, Invocation};
use oso_core::editor::Editor;
use oso_core::sum_tree::Bias;

use super::Dispatcher;
use crate::commands::agent_sessions::{SLASH_LIST, begun_argument};

/// The command whose prompt lists the slash commands.
const SLASH_COMMAND: &str = "native-composer-command";
/// The most of a composer's start read for the `/model` or `/effort` it may begin with.
const ARGUMENT_BYTES: usize = 16;

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
        let Some(command) = self.registry.lookup("native-composer-command") else {
            return Effects::default();
        };
        self.run_or_read(editor, command, Args::NONE, Invocation::default())
            .0
    }

    /// After command `id` ran: `/model` or `/effort` chosen in the slash commands' prompt
    /// opens its argument's prompt (part 3c).
    pub(super) fn complete_argument(&mut self, editor: &mut Editor, id: CommandId) -> Effects {
        let slash = self.registry.get(id).map(|command| command.name) == Some(SLASH_COMMAND);
        if !slash || editor.prompt().is_some() {
            return Effects::default();
        }
        match self.argument_of(editor) {
            Some(argument) => {
                (self.run_or_read(editor, argument, Args::NONE, Invocation::default())).0
            }
            None => Effects::default(),
        }
    }

    /// The command to run in place of `id`: the slash commands' on a `/model` or `/effort` the
    /// selected composer begins with completes its argument instead (part 3c).
    pub(super) fn argument_prompt(&self, editor: &Editor, id: CommandId) -> CommandId {
        let slash = self.registry.get(id).map(|command| command.name) == Some(SLASH_COMMAND);
        slash
            .then(|| self.argument_of(editor))
            .flatten()
            .unwrap_or(id)
    }

    /// The command that completes the argument of the `/model` or `/effort` the selected
    /// composer begins with, if the host listed any for it.
    fn argument_of(&self, editor: &Editor) -> Option<CommandId> {
        let buffer = editor.buffer(editor.window().buffer())?;
        if buffer.mode() != Mode::Composer {
            return None;
        }
        let text = buffer.text();
        let end = text.clip_offset(text.len().min(ARGUMENT_BYTES), Bias::Left);
        let start: String = text.chunks_in_range(0..end).collect();
        let (_, command, list) = begun_argument(&start)?;
        if editor.listed_candidates(list).is_empty() {
            return None;
        }
        self.registry.lookup(command)
    }
}
