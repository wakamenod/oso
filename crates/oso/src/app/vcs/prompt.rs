//! The prompts and questions git's heads open over a status (ADR 0127, steps 2 to 3b): opening
//! a command's prompt as its keys would, asking a `y` or `n` question on one line, and telling
//! whether the person is still where a head was asked, so that a list the run thread gives back
//! late opens nothing over what they do now.
//!
//! Must not depend on: key presses or how a frame is drawn; this opens prompts and runs nothing.

use oso_core::buffer::BufferId;
use oso_core::minibuffer::PromptId;

use super::super::App;
use crate::commands::review::one_line;

/// The most characters of a question shown.
const MAX_QUESTION: usize = 400;

impl App {
    /// Opens `command`'s prompt as its keys would; the prompt it opened, if one did.
    pub(super) fn open_host_prompt(&mut self, command: &str) -> Option<PromptId> {
        let before = self.editor.prompt().map(oso_core::minibuffer::Prompt::id);
        let effects = self.dispatcher.run_from_host(&mut self.editor, command);
        if effects.redraw {
            self.request_redraw();
        }
        let opened = self.editor.prompt().map(oso_core::minibuffer::Prompt::id);
        opened.filter(|id| Some(*id) != before)
    }

    /// Asks `question` on one line, its `y` running `yes`; the question's prompt, or `None`
    /// once the echo area says why it could not be asked.
    pub(super) fn ask_bound(
        &mut self,
        question: &str,
        yes: &'static str,
        verbs: [&'static str; 2],
    ) -> Option<PromptId> {
        let question = one_line(question, MAX_QUESTION);
        let asked = self
            .editor
            .ask_with(format!("{question} (y or n)"), yes, verbs);
        self.request_redraw();
        match asked {
            Ok(prompt) => Some(prompt),
            Err(e) => {
                self.editor.message(format!("Cannot ask: {e}"));
                None
            }
        }
    }

    /// Whether the person is still where a head over `status`, a status or another buffer such
    /// as an issue's, was asked: the selected window shows it and no prompt is open.
    pub(super) fn still_at(&self, status: BufferId) -> bool {
        self.editor.current().0.buffer() == status && self.editor.prompt().is_none()
    }
}
