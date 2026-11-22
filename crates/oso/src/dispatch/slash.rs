//! A native session's slash commands as the person types (ADR 0165, step 2d, the author's Q1
//! (a)): `/` typed as an empty composer's first character opens `native-composer-command`'s
//! prompt, as `C-c /` does; `C-g` there leaves the `/` typed. Not an agent's `/`, nor one the
//! host runs.
//!
//! Must not depend on: `app` or winit; the host lists the commands the prompt completes over.

use oso_core::buffer::Mode;
use oso_core::command::{Args, CommandId, Effects, Invocation};
use oso_core::editor::Editor;

use super::Dispatcher;
use crate::commands::agent_sessions::SLASH_LIST;

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
}
