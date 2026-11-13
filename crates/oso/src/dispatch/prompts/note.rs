//! The prompts of a command that sends to an agent session (ADR 0121): the session, read with
//! completion over a list the host keeps, then a note, unless the selected window shows a
//! region, which is what is sent then.
//!
//! Must not depend on: `app` or winit; key sequences stay testable without a window.

use oso_core::command::{CommandId, Effects, Prefix};
use oso_core::editor::Editor;

use super::super::Dispatcher;
use super::{Reading, TextPrompt, Then};

/// What a `ListedAndNoteUnlessRegion` command reads.
#[derive(Clone, Copy)]
pub(super) struct ListedAndNote {
    pub(super) list: &'static str,
    pub(super) prompt: &'static str,
    pub(super) history: &'static str,
    pub(super) note: TextPrompt,
}

impl Dispatcher {
    /// Opens the completion over the host's `list` for the command `id`. Its answer runs the
    /// command when the selected window shows a region now, and otherwise opens the note's
    /// prompt, whose answer runs it with both.
    pub(super) fn read_listed_and_note(
        &mut self,
        editor: &mut Editor,
        id: CommandId,
        prefix: Option<Prefix>,
        read: ListedAndNote,
    ) -> Effects {
        // A region shows, as the command reads it: an active mark and a selection with text.
        let selections = &editor.window().selections;
        let region = selections.mark_active().is_active()
            && selections
                .iter()
                .any(|selection| !selection.range().is_empty());
        let items = editor.listed_candidates(read.list);
        match editor.open_completion(read.prompt, read.history, items) {
            Ok(prompt) => self.reading.push(Reading {
                then: if region {
                    Then::Run
                } else {
                    Then::DirectoryThenText(read.note)
                },
                ..Reading::run(id, prefix, prompt)
            }),
            Err(error) => editor.prompt_says(error.to_string()),
        }
        Effects {
            redraw: true,
            ..Effects::default()
        }
    }
}
