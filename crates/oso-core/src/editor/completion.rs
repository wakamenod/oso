//! Completion in a prompt: its candidates, matched again after each command, the selected
//! one, and what `TAB` and `RET` take from it (ADR 0076).
//!
//! Must not depend on: `command` or `keymap`, or the host's window.
//!
//! The candidates' order is fixed as the prompt opens, from its history unless a buffer
//! that may hold secrets is shown, as that history is then neither read nor recorded
//! (ADR 0072). What is typed is matched once a command ends, so a key that edits the
//! answer and one that moves through the candidates cost one match at most.

use std::sync::Arc;

use super::Editor;
use crate::buffer::Replacement;
use crate::complete::{Candidate, Candidates, MAX_INPUT_BYTES};
use crate::minibuffer::{Kind, PromptError, PromptId};

/// How a prompt that completes orders its candidates, and what it takes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CompletionStyle {
    /// The candidates keep the order given, as buffers do, most recent first (ADR 0088),
    /// instead of ADR 0076's history, then length, then alphabetical.
    pub given_order: bool,
    /// Any text is an answer, not only a candidate's: a buffer or a file that need not exist
    /// yet (ADR 0087, ADR 0088).
    pub takes_input: bool,
    /// The float shows the selected candidate beside the list, as `C-x b` does (ADR 0088).
    pub preview: bool,
}

impl Editor {
    /// Opens a prompt that says `label` and completes over `items`, kept in the history
    /// `history`. They show in its order first, unless a buffer that may hold secrets is
    /// shown now.
    ///
    /// # Errors
    ///
    /// [`PromptError::TooDeep`] with as many prompts open as may be. Nothing changes.
    pub fn open_completion(
        &mut self,
        label: impl Into<String>,
        history: &'static str,
        items: Arc<[Candidate]>,
    ) -> Result<PromptId, PromptError> {
        self.open_completion_as(label, history, items, CompletionStyle::default())
    }

    /// Opens a prompt that completes, as [`Editor::open_completion`] does, in `style`.
    ///
    /// # Errors
    ///
    /// As [`Editor::open_completion`].
    pub fn open_completion_as(
        &mut self,
        label: impl Into<String>,
        history: &'static str,
        items: Arc<[Candidate]>,
        style: CompletionStyle,
    ) -> Result<PromptId, PromptError> {
        let id = self.open_prompt(label, Kind::Complete { history })?;
        let records = self
            .prompts
            .last()
            .is_some_and(|prompt| !prompt.around_secret);
        let mut candidates = if style.given_order {
            Candidates::in_given_order(items)
        } else if records {
            Candidates::new(items, self.histories.entries(history))
        } else {
            Candidates::new(items, [])
        };
        if style.takes_input {
            candidates.take_input();
        }
        if style.preview {
            candidates.show_preview();
        }
        let revision = self.prompt_revision().unwrap_or_default();
        if let Some(prompt) = self.prompts.last_mut() {
            prompt.completion = Some((candidates, revision));
        }
        self.relayout_prompt();
        Ok(id)
    }

    /// Keeps the answer of the prompt `id` out of its history: a control socket request opened
    /// it, so it is not the person's (ADR 0076).
    pub fn keep_out_of_history(&mut self, id: PromptId) {
        if let Some(prompt) = self.prompts.iter_mut().find(|prompt| prompt.id == id) {
            prompt.from_socket = true;
        }
    }

    /// The revision of the innermost prompt's buffer.
    fn prompt_revision(&self) -> Option<u64> {
        let prompt = self.prompts.last()?;
        Some(self.buffer(prompt.buffer)?.revision())
    }

    /// Matches the innermost prompt's answer against its candidates again, if it changed
    /// since they were last matched. The dispatcher calls it as each command ends, so what a
    /// command typed is matched once.
    pub fn match_completion(&mut self) {
        let Some(prompt) = self.prompts.last() else {
            return;
        };
        let Some((_, seen)) = &prompt.completion else {
            return;
        };
        let Some(buffer) = self.buffer(prompt.buffer) else {
            return;
        };
        if buffer.revision() == *seen {
            return;
        }
        let revision = buffer.revision();
        // An answer too long to match, such as a whole file yanked into it, is never read.
        let text = (buffer.text().len() <= MAX_INPUT_BYTES).then(|| buffer.text().to_string());
        if let Some((candidates, seen)) = self.completion_mut() {
            match text {
                Some(text) => candidates.set_input(&text),
                None => candidates.set_too_long(),
            }
            *seen = revision;
        }
        // Which also keeps the selection in view, in the rows the float has now.
        self.relayout_prompt();
    }

    /// The innermost prompt's candidates, if it completes.
    fn completion_mut(&mut self) -> Option<&mut (Candidates, u64)> {
        self.prompts.last_mut()?.completion.as_mut()
    }

    /// How many candidate rows the innermost prompt shows.
    pub fn candidate_rows(&self) -> usize {
        self.prompt_layout()
            .map_or(0, |layout| layout.regions.candidates.rows)
    }

    /// Scrolls the innermost prompt's candidates so that the selected one shows in the rows
    /// its float has.
    pub(super) fn keep_selection_in_view(&mut self) {
        let rows = self.candidate_rows();
        if let Some((candidates, _)) = self.completion_mut() {
            candidates.keep_in_view(rows);
        }
    }

    /// Selects the candidate `by` places after the selected one, or before it for a negative
    /// `by`, stopping at the first and the last.
    ///
    /// # Errors
    ///
    /// [`PromptError::NoPrompt`] unless the innermost prompt completes, and
    /// [`PromptError::Invalid`] where nothing matches.
    pub fn move_completion(&mut self, by: isize) -> Result<(), PromptError> {
        self.with_candidates(|candidates| candidates.move_by(by))
    }

    /// Selects the first candidate, or the last if `last`.
    ///
    /// # Errors
    ///
    /// As [`Editor::move_completion`].
    pub fn select_completion_end(&mut self, last: bool) -> Result<(), PromptError> {
        self.with_candidates(|candidates| {
            candidates.select(if last { usize::MAX } else { 0 });
        })
    }

    /// Moves the selection by `pages` of the rows shown, as `C-v` and `M-v` do.
    ///
    /// # Errors
    ///
    /// As [`Editor::move_completion`].
    pub fn page_completion(&mut self, pages: isize) -> Result<(), PromptError> {
        let rows = isize::try_from(self.candidate_rows().max(1)).unwrap_or(isize::MAX);
        self.move_completion(pages.saturating_mul(rows))
    }

    fn with_candidates(&mut self, f: impl FnOnce(&mut Candidates)) -> Result<(), PromptError> {
        // What was typed last is matched before the selection moves over it.
        self.match_completion();
        let (candidates, _) = self.completion_mut().ok_or(PromptError::NoPrompt)?;
        if candidates.is_empty() {
            return Err(PromptError::Invalid);
        }
        f(candidates);
        self.keep_selection_in_view();
        Ok(())
    }

    /// Puts the selected candidate in the innermost prompt's answer, as `TAB` does.
    ///
    /// # Errors
    ///
    /// As [`Editor::move_completion`].
    pub fn insert_completion(&mut self) -> Result<(), PromptError> {
        let text = self.selected_completion()?;
        let Some(prompt) = self.prompts.last() else {
            return Err(PromptError::NoPrompt);
        };
        let id = prompt.buffer;
        let end = self.buffer(id).map_or(0, |buffer| buffer.text().len());
        self.apply_edits(
            id,
            &[Replacement {
                range: 0..end,
                text: &text,
            }],
        )
        .map_err(|_| PromptError::NoPrompt)?;
        self.match_completion();
        Ok(())
    }

    /// The text of the innermost prompt's selected candidate, with what was typed last
    /// matched first.
    ///
    /// # Errors
    ///
    /// As [`Editor::move_completion`].
    pub(super) fn selected_completion(&mut self) -> Result<String, PromptError> {
        self.match_completion();
        let prompt = self.prompts.last().ok_or(PromptError::NoPrompt)?;
        let candidates = prompt.candidates().ok_or(PromptError::NoPrompt)?;
        candidates
            .selected_candidate()
            .map(|candidate| candidate.text().to_owned())
            .ok_or(PromptError::Invalid)
    }
}
