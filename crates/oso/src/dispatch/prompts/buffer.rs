//! The prompts that read a buffer, for `C-x b` and `C-x k` (ADR 0088), or a recent file, for
//! `C-x C-r` (ADR 0148), completing over the lists `dispatch::buffers` makes.
//!
//! Must not depend on: `app` or winit; key sequences stay testable without a window.

use oso_core::command::{BufferPurpose, CommandId, Effects, ExecuteError, Prefix};
use oso_core::editor::{CompletionStyle, Editor};

use super::super::{Dispatcher, buffers};
use super::Reading;

/// What `ArgSpec::Buffer` asks for, or with no purpose `ArgSpec::RecentFile` (ADR 0148).
pub(super) struct ReadBuffer {
    pub(super) prompt: &'static str,
    pub(super) history: &'static str,
    pub(super) purpose: Option<BufferPurpose>,
}

impl Dispatcher {
    /// Opens the prompt that reads a buffer for the command `id`, or a recent file for `C-x C-r`,
    /// unless a prompt is selected, whose window always shows its own buffer, so these refuse
    /// there before asking anything (ADR 0072, ADR 0088, ADR 0148).
    pub(super) fn read_buffer(
        &mut self,
        editor: &mut Editor,
        id: CommandId,
        prefix: Option<Prefix>,
        read: ReadBuffer,
    ) -> (Effects, Result<(), ExecuteError>) {
        let effects = Effects {
            redraw: true,
            ..Effects::default()
        };
        if editor.prompt().is_some() {
            editor.prompt_says("Not in the minibuffer");
            return (effects, Ok(()));
        }
        let (style, items) = match read.purpose {
            // The buffers, most recent first; showing takes any name.
            Some(purpose) => (
                CompletionStyle {
                    given_order: true,
                    takes_input: purpose == BufferPurpose::Switch,
                    preview: purpose == BufferPurpose::Switch,
                },
                buffers::candidates(editor, purpose),
            ),
            // The recent files: only a listed one, and none previewed, as none need be open.
            None => (
                CompletionStyle {
                    given_order: true,
                    ..CompletionStyle::default()
                },
                buffers::recent_candidates(editor),
            ),
        };
        match editor.open_completion_as(read.prompt, read.history, items, style) {
            Ok(prompt) => self.reading.push(Reading::run(id, prefix, prompt)),
            Err(error) => editor.prompt_says(error.to_string()),
        }
        (effects, Ok(()))
    }
}
