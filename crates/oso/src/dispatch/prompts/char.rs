//! The prompt that reads one character for a command, as Emacs's `read-char-from-minibuffer`
//! does for `zap-to-char` (ADR 0190): a prompt of any text, in no history, that ends as soon as
//! typed text puts a character in it.
//!
//! Must not depend on: `app` or winit; key sequences stay testable without a window.

use oso_core::command::{CommandId, Effects, ExecuteError, Prefix};
use oso_core::editor::Editor;
use oso_core::minibuffer::Kind;

use super::super::Dispatcher;
use super::{Reading, Then};

impl Dispatcher {
    /// Opens the prompt that reads one character for the command `id`, in no history, which
    /// ends as soon as it holds one ([`Dispatcher::end_char_prompt`]), and keeps the command
    /// waiting for it with its prefix argument.
    pub(super) fn read_char(
        &mut self,
        editor: &mut Editor,
        id: CommandId,
        prefix: Option<Prefix>,
        prompt: &'static str,
    ) -> (Effects, Result<(), ExecuteError>) {
        match editor.open_prompt(prompt, Kind::Text { history: None }) {
            Ok(prompt) => self.reading.push(Reading {
                then: Then::Char,
                ..Reading::run(id, prefix, prompt)
            }),
            // The innermost prompt is where the person looks.
            Err(error) => editor.prompt_says(error.to_string()),
        }
        let effects = Effects {
            redraw: true,
            ..Effects::default()
        };
        (effects, Ok(()))
    }

    /// Whether the innermost prompt reads one character for a command (ADR 0190).
    pub(in crate::dispatch) fn reads_char(&self, editor: &Editor) -> bool {
        editor.prompt().is_some_and(|prompt| {
            self.reading
                .iter()
                .any(|reading| reading.prompt == prompt.id() && matches!(reading.then, Then::Char))
        })
    }

    /// Gives the answer of the innermost prompt if it reads one character for a command and
    /// now holds one, as Emacs's `read-char-from-minibuffer` ends at the first character typed
    /// (ADR 0190).
    pub(super) fn end_char_prompt(&mut self, editor: &mut Editor) {
        let Some(prompt) = editor.prompt() else {
            return;
        };
        let typed = editor
            .buffer(prompt.buffer())
            .is_some_and(|buffer| !buffer.text().is_empty());
        if typed && self.reads_char(editor) {
            // It reads any text, so it takes what it holds.
            let _ = editor.submit_prompt();
        }
    }
}
