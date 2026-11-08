//! Prompts on the editor: opening and closing them, their answers and histories, and
//! the `C-g` chain (ADR 0072).
//!
//! Must not depend on: `command` or `keymap`, or the host's window.
//!
//! A prompt's window goes after the tiled ones in [`Editor::windows`] and is the selected
//! one while it is open, so commands act on its buffer unchanged (ADR 0008). The window
//! it came from is the one beneath, which the modeline speaks of and the wheel scrolls.
//! Only the innermost prompt shows. Closing one drops its window and its buffer, so no
//! text of it stays behind.

use super::Editor;
use crate::buffer::{Buffer, Replacement};
use crate::frame;
use crate::minibuffer::{self, Kind, MAX_DEPTH, Outcome, Prompt, PromptError, PromptLayout};
use crate::window::{ViewSize, Window};

/// What one `C-g` did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quit {
    /// It closed the innermost prompt, and ran nothing.
    Prompt,
    /// Nothing was in progress.
    Nothing,
}

impl Editor {
    /// Opens a prompt that says `label` and reads `kind`, over the selected window. Its
    /// answer enters its history unless a buffer that may hold secrets is shown now.
    ///
    /// # Errors
    ///
    /// [`PromptError::TooDeep`] with [`MAX_DEPTH`] open already. Nothing changes.
    pub fn open_prompt(&mut self, label: impl Into<String>, kind: Kind) -> Result<(), PromptError> {
        if self.prompts.len() >= MAX_DEPTH {
            return Err(PromptError::TooDeep);
        }
        let depth = self.prompts.len() + 1;
        let mut label = label.into();
        if kind.is_question() {
            label.push_str(" (y or n)");
        }
        if depth > 1 {
            label = format!("[{depth}] {label}");
        }
        let records = !self.shows_secret();
        let passphrase = kind == Kind::Passphrase;
        let buffer = self.add_buffer(Buffer::prompt(format!(" *Minibuf-{depth}*"), passphrase));
        let beneath = self.selected;
        self.windows.push(Window::new(buffer));
        self.selected = self.windows.len() - 1;
        self.prompts.push(Prompt {
            label,
            kind,
            buffer,
            beneath,
            message: None,
            records,
            walk: None,
            typed: None,
        });
        self.fit_prompt_window();
        Ok(())
    }

    /// Asks `question`, answered by `y` or `n`: `y` runs the command `yes`, and `n` runs
    /// nothing. That command checks again what is true by then.
    ///
    /// # Errors
    ///
    /// [`PromptError::TooDeep`] with [`MAX_DEPTH`] open already.
    pub fn ask(
        &mut self,
        question: impl Into<String>,
        yes: &'static str,
    ) -> Result<(), PromptError> {
        self.open_prompt(question, Kind::Question { yes, no: None })
    }

    /// The open prompts, the outermost first.
    pub fn prompts(&self) -> &[Prompt] {
        &self.prompts
    }

    /// The innermost prompt, which shows and takes the keys.
    pub fn prompt(&self) -> Option<&Prompt> {
        self.prompts.last()
    }

    /// The tiled window beneath every prompt, which is the selected window with none
    /// open, and the buffer it shows.
    pub fn tiled(&self) -> (&Window, &Buffer) {
        let window = &self.windows[self.tiled_index()];
        (window, self.shown(window))
    }

    pub(super) fn tiled_index(&self) -> usize {
        self.prompts
            .first()
            .map_or(self.selected, |prompt| prompt.beneath)
    }

    /// How many windows are tiled: those before the prompts'.
    pub(super) fn tiled_count(&self) -> usize {
        self.windows.len() - self.prompts.len()
    }

    /// Gives the innermost prompt's answer to whoever reads [`Editor::take_prompt_outcome`],
    /// records it in its history, and closes the prompt.
    ///
    /// # Errors
    ///
    /// [`PromptError::NoPrompt`] if the innermost prompt is a question or there is none,
    /// and [`PromptError::Invalid`] for a number that is not one, which the prompt then
    /// says, staying open.
    pub fn submit_prompt(&mut self) -> Result<(), PromptError> {
        let Some(prompt) = self.prompts.last().filter(|p| !p.kind.is_question()) else {
            return Err(PromptError::NoPrompt);
        };
        let text = self
            .buffer(prompt.buffer)
            .map(|buffer| buffer.text().to_string())
            .unwrap_or_default();
        if matches!(prompt.kind, Kind::Number { .. }) && text.trim().parse::<i64>().is_err() {
            self.prompt_says("Not a number");
            return Err(PromptError::Invalid);
        }
        let depth = self.prompts.len();
        if let Some(prompt) = self.close_innermost()
            && prompt.records
            && let Some(name) = prompt.kind.history()
        {
            self.histories.record(name, text.trim());
        }
        self.outcome = Some(Outcome::Answered { depth, text });
        Ok(())
    }

    /// Answers the innermost prompt's question and closes it. `y` leaves its command for
    /// [`Editor::take_prompt_outcome`]; `n` leaves its other one, if any, and forgets a
    /// refused exit.
    ///
    /// # Errors
    ///
    /// [`PromptError::NoPrompt`] unless the innermost prompt is a question.
    pub fn answer_question(&mut self, yes: bool) -> Result<(), PromptError> {
        let Some(Kind::Question { yes: on_yes, no }) = self.prompt().map(Prompt::kind) else {
            return Err(PromptError::NoPrompt);
        };
        self.close_innermost();
        if !yes {
            self.reset_exit_guard();
        }
        self.outcome = if yes { Some(on_yes) } else { no }.map(Outcome::Run);
        Ok(())
    }

    /// How the last prompt to end ended, once, for the dispatcher.
    pub fn take_prompt_outcome(&mut self) -> Option<Outcome> {
        self.outcome.take()
    }

    /// Puts the answer `older` steps from where the innermost prompt's history walk is in
    /// its buffer, as `M-p` and `M-n` do. Past the newest, what was typed comes back.
    ///
    /// # Errors
    ///
    /// [`PromptError::NoPrompt`] with no history to walk, and [`PromptError::Invalid`] at
    /// its end. Nothing changes.
    pub fn walk_history(&mut self, older: bool) -> Result<(), PromptError> {
        let Some(prompt) = self.prompts.last() else {
            return Err(PromptError::NoPrompt);
        };
        let name = prompt
            .kind
            .history()
            .filter(|_| prompt.records)
            .ok_or(PromptError::NoPrompt)?;
        let next = match (prompt.walk, older) {
            (None, true) => Some(0),
            (Some(at), true) => Some(at + 1),
            (Some(0), false) => None,
            (Some(at), false) => Some(at - 1),
            (None, false) => return Err(PromptError::Invalid),
        };
        let text = match next {
            Some(index) => self.histories.get(name, index).map(str::to_owned),
            None => Some(prompt.typed.clone().unwrap_or_default()),
        }
        .ok_or(PromptError::Invalid)?;
        let id = prompt.buffer;
        let typed = match prompt.walk {
            None => self.buffer(id).map(|buffer| buffer.text().to_string()),
            Some(_) => prompt.typed.clone(),
        };
        let end = self.buffer(id).map_or(0, |buffer| buffer.text().len());
        self.apply_edits(
            id,
            &[Replacement {
                range: 0..end,
                text: &text,
            }],
        )
        .map_err(|_| PromptError::NoPrompt)?;
        if let Some(prompt) = self.prompts.last_mut() {
            prompt.walk = next;
            prompt.typed = typed;
        }
        Ok(())
    }

    /// Shows `text` in the innermost prompt until the next key, as a message from a key
    /// typed there does.
    pub fn prompt_says(&mut self, text: impl Into<String>) {
        if let Some(prompt) = self.prompts.last_mut() {
            prompt.message = Some(text.into());
        }
        self.fit_prompt_window();
    }

    /// Where the innermost prompt's parts go, if one is open.
    pub fn prompt_layout(&self) -> Option<PromptLayout> {
        let prompt = self.prompts.last()?;
        let inner = frame::float_regions(&self.frame, 1).inner.columns;
        let rows = minibuffer::label_rows(&prompt.label, prompt.kind, inner);
        let regions = frame::float_regions(&self.frame, rows);
        Some(minibuffer::layout(
            regions,
            &prompt.label,
            prompt.kind,
            prompt.message.as_deref(),
        ))
    }

    /// Gives the innermost prompt's window the size of where its answer is typed.
    pub(super) fn fit_prompt_window(&mut self) {
        let Some(layout) = self.prompt_layout() else {
            return;
        };
        let size = ViewSize::new(1, layout.input.columns);
        self.windows[self.selected].set_size(size);
    }

    /// Closes the innermost prompt, with its window and its buffer, and selects the window it
    /// came from again.
    fn close_innermost(&mut self) -> Option<Prompt> {
        let prompt = self.prompts.pop()?;
        self.windows.pop();
        self.selected = prompt.beneath;
        let id = prompt.buffer;
        self.buffers.retain(|(other, _)| *other != id);
        self.fit_prompt_window();
        Some(prompt)
    }

    /// Does what one `C-g` does once a pending prefix is dealt with: the first that applies of
    /// the steps of ADR 0072, which M2-13 (the region), M2-20 (isearch) and M2-21 (extra
    /// cursors) add their part to, in this order. Closing a question runs nothing and forgets
    /// a refused exit.
    pub fn quit(&mut self) -> Quit {
        if let Some(prompt) = self.prompts.last() {
            if prompt.kind.is_question() {
                self.reset_exit_guard();
            }
            self.close_innermost();
            return Quit::Prompt;
        }
        Quit::Nothing
    }

    /// Clears what a key typed in the innermost prompt said, as the next key does.
    pub(super) fn clear_prompt_message(&mut self) {
        if let Some(prompt) = self.prompts.last_mut()
            && prompt.message.take().is_some()
        {
            self.fit_prompt_window();
        }
    }

    /// Whether a message said now goes in the innermost prompt: one from a command run while
    /// it is selected.
    pub(super) fn message_goes_in_prompt(&self) -> bool {
        self.command_left_secret.is_some() && !self.prompts.is_empty()
    }
}
