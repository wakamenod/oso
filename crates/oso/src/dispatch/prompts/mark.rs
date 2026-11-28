//! The prompt that reads a mark for `C-.` (ADR 0071, ADR 0088): the primary cursor's mark and
//! its ring, listed by the editor.
//!
//! Must not depend on: `app` or winit; key sequences stay testable without a window.

use oso_core::command::{CommandId, Effects, ExecuteError, Prefix};
use oso_core::editor::Editor;
use oso_core::minibuffer::PromptError;

use super::super::Dispatcher;
use super::Reading;

impl Dispatcher {
    /// Opens the prompt that reads a mark for the command `id`, `C-.`, unless a prompt is
    /// selected, whose own buffer is no list of marks and may be a passphrase's (ADR 0088).
    pub(super) fn read_mark(
        &mut self,
        editor: &mut Editor,
        id: CommandId,
        prefix: Option<Prefix>,
        prompt: &'static str,
    ) -> (Effects, Result<(), ExecuteError>) {
        let effects = Effects {
            redraw: true,
            ..Effects::default()
        };
        if editor.prompt().is_some() {
            editor.prompt_says("Not in the minibuffer");
            return (effects, Ok(()));
        }
        match editor.open_mark_list(prompt) {
            Ok(prompt) => self.reading.push(Reading::run(id, prefix, prompt)),
            Err(PromptError::Invalid) => editor.message("No mark set in this buffer"),
            Err(error) => editor.prompt_says(error.to_string()),
        }
        (effects, Ok(()))
    }
}
