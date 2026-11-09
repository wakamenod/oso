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
use crate::minibuffer::{
    self, Kind, MAX_DEPTH, Outcome, Prompt, PromptError, PromptId, PromptLayout,
};
use crate::window::{ViewSize, Window};

/// What one `C-g` did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quit {
    /// It hid the selected window's region, on every cursor (ADR 0071).
    Region,
    /// It closed the innermost prompt, and ran nothing.
    Prompt,
    /// It took a failing part or a running search off isearch, back to the last string
    /// that matched, and the search goes on (ADR 0082).
    SearchStep,
    /// It ended isearch, and put the cursors and the view back as they were.
    Search,
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
    pub fn open_prompt(
        &mut self,
        label: impl Into<String>,
        kind: Kind,
    ) -> Result<PromptId, PromptError> {
        if self.prompts.len() >= MAX_DEPTH {
            return Err(PromptError::TooDeep);
        }
        // A prompt over a search ends it, as a key it does not bind does (ADR 0082).
        self.isearch_abandon();
        let depth = self.prompts.len() + 1;
        let from_agent = self.agent_opens_prompts();
        let mut label = label.into();
        if kind.is_question() {
            label.push_str(minibuffer::QUESTION_END);
        }
        if from_agent {
            label.insert_str(0, super::agent::AGENT_LABEL);
        }
        if depth > 1 {
            label = format!("[{depth}] {label}");
        }
        let around_secret = self.shows_secret();
        let passphrase = kind == Kind::Passphrase;
        let buffer = self.add_buffer(Buffer::prompt(format!(" *Minibuf-{depth}*"), passphrase));
        let beneath = self.selected;
        self.windows.push(Window::new(buffer));
        self.selected = self.windows.len() - 1;
        let id = PromptId(self.next_prompt);
        self.next_prompt += 1;
        self.prompts.push(Prompt {
            id,
            label,
            kind,
            buffer,
            beneath,
            message: None,
            message_withheld: false,
            around_secret,
            from_socket: false,
            walk: None,
            typed: None,
            from_agent,
            completion: None,
        });
        self.relayout_prompt();
        Ok(id)
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
    ) -> Result<PromptId, PromptError> {
        self.ask_with(question, yes, minibuffer::VERBS)
    }

    /// Asks `question` as [`Self::ask`] does, with `verbs` saying what `y` and `n` do after
    /// their keys, such as `exit` and `stay` (ADR 0075).
    ///
    /// # Errors
    ///
    /// [`PromptError::TooDeep`] with [`MAX_DEPTH`] open already.
    pub fn ask_with(
        &mut self,
        question: impl Into<String>,
        yes: &'static str,
        verbs: [&'static str; 2],
    ) -> Result<PromptId, PromptError> {
        self.open_prompt(question, Kind::Question { yes, verbs })
    }

    /// The open prompts, the outermost first.
    pub fn prompts(&self) -> &[Prompt] {
        &self.prompts
    }

    /// The innermost prompt, which shows and takes the keys.
    pub fn prompt(&self) -> Option<&Prompt> {
        self.prompts.last()
    }

    /// The open prompts' windows, the outermost first, as [`Editor::prompts`] lists them.
    pub fn prompt_windows(&self) -> impl Iterator<Item = &Window> {
        self.windows[self.tiled_count()..].iter()
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
    /// and [`PromptError::Invalid`] for a number that is not one, or a prompt that completes
    /// with nothing matching, which the prompt then says, staying open. A prompt that
    /// completes gives its selected candidate (ADR 0076).
    pub fn submit_prompt(&mut self) -> Result<(), PromptError> {
        self.submit(false)
    }

    /// Gives what is typed in the innermost prompt, whatever candidate is selected, as `M-RET`
    /// does in a prompt that takes any text (`vertico-exit-input`). Elsewhere it is
    /// [`Editor::submit_prompt`].
    ///
    /// # Errors
    ///
    /// As [`Editor::submit_prompt`].
    pub fn submit_typed(&mut self) -> Result<(), PromptError> {
        self.submit(true)
    }

    fn submit(&mut self, typed: bool) -> Result<(), PromptError> {
        let completes = matches!(self.prompt().map(Prompt::kind), Some(Kind::Complete { .. }));
        // What was typed last is matched first.
        self.match_completion();
        // A prompt that reads a path gives a path (ADR 0087).
        if let Some(given) = self.submit_path(typed) {
            return given;
        }
        // Around a secret no history orders the list, so its first candidate is no choice: an
        // empty answer takes one only once the person moved to it (ADR 0076).
        // A list in an order of its own, as buffers most recent first, needs no history to make
        // its first a choice (ADR 0088).
        let unchosen = self.prompt().is_some_and(|prompt| {
            prompt.around_secret
                && prompt.candidates().is_some_and(|candidates| {
                    candidates.defaulted() && !candidates.in_order_given()
                })
        });
        // What is typed goes as it is only in a prompt that takes any text, and only once
        // something is typed: otherwise `M-RET` is `RET`.
        let typed_given = typed
            && self.prompt().is_some_and(|prompt| {
                prompt
                    .candidates()
                    .is_some_and(crate::complete::Candidates::takes_input)
                    && self
                        .buffer(prompt.buffer)
                        .is_some_and(|buffer| !buffer.text().is_empty())
            });
        let typed = typed_given;
        if completes && unchosen && !typed {
            self.prompt_says("Type a name, or move to one");
            return Err(PromptError::Invalid);
        }
        let (takes_input, on_input) = self
            .prompt()
            .and_then(Prompt::candidates)
            .map_or((false, false), |candidates| {
                (candidates.takes_input(), candidates.on_input())
            });
        let selected = if completes && !on_input && !(typed && takes_input) {
            match self.selected_completion() {
                Ok(text) => Some(text),
                // A prompt that takes any text gives what is typed (ADR 0087, ADR 0088).
                Err(PromptError::Invalid) if takes_input => None,
                Err(PromptError::Invalid) => {
                    self.prompt_says("No match");
                    return Err(PromptError::Invalid);
                }
                Err(error) => return Err(error),
            }
        } else {
            None
        };
        let Some(prompt) = self.prompts.last().filter(|p| !p.kind.is_question()) else {
            return Err(PromptError::NoPrompt);
        };
        let text = selected.unwrap_or_else(|| {
            self.buffer(prompt.buffer)
                .map(|buffer| buffer.text().to_string())
                .unwrap_or_default()
        });
        if matches!(prompt.kind, Kind::Number { .. }) && minibuffer::parse_number(&text).is_none() {
            self.prompt_says("Not a number");
            return Err(PromptError::Invalid);
        }
        self.give_answer(text)
    }

    /// Gives `text` as the innermost prompt's answer, records it in its history, and closes
    /// the prompt.
    ///
    /// # Errors
    ///
    /// [`PromptError::NoPrompt`] if the innermost prompt is a question or there is none.
    pub(super) fn give_answer(&mut self, text: String) -> Result<(), PromptError> {
        let Some(prompt) = self.prompts.last().filter(|p| !p.kind.is_question()) else {
            return Err(PromptError::NoPrompt);
        };
        let id = prompt.id;
        // Also not if a buffer that may hold secrets came beneath it since it opened.
        let records = !prompt.around_secret && !prompt.from_socket && !self.shows_secret();
        if let Some(prompt) = self.close_innermost()
            && records
            && let Some(name) = prompt.kind.history()
            // An empty name is no history, as `C-.`'s rows are the buffer's text (ADR 0088).
            && !name.is_empty()
        {
            self.histories.record(name, text.trim());
        }
        self.outcome = Some(Outcome::Answered { prompt: id, text });
        Ok(())
    }

    /// Answers the innermost prompt's question and closes it. `y` leaves its command for
    /// [`Editor::take_prompt_outcome`]; `n` leaves nothing, and forgets a refused exit.
    ///
    /// # Errors
    ///
    /// [`PromptError::NoPrompt`] unless the innermost prompt is a question.
    pub fn answer_question(&mut self, yes: bool) -> Result<(), PromptError> {
        let Some(Kind::Question { yes: command, .. }) = self.prompt().map(Prompt::kind) else {
            return Err(PromptError::NoPrompt);
        };
        self.close_innermost();
        if yes {
            self.outcome = Some(Outcome::Run(command));
        } else {
            self.reset_exit_guard();
        }
        Ok(())
    }

    /// How the last prompt to end ended, once, for the dispatcher.
    pub fn take_prompt_outcome(&mut self) -> Option<Outcome> {
        self.outcome.take()
    }

    /// Each history's name and answers, the newest first, which `oso dev` carries to the
    /// instance that replaces this one (ADR 0090).
    pub fn histories(&self) -> impl Iterator<Item = (&str, impl Iterator<Item = &str>)> {
        self.histories.all()
    }

    /// Puts answers another instance's history `name` held behind what this one holds, the
    /// newest first, leaving out what a history would not keep (ADR 0090).
    pub fn carry_history(&mut self, name: &str, newest_first: impl IntoIterator<Item = String>) {
        self.histories.extend_older(name, newest_first);
    }

    /// Puts the answer `older` steps from where the innermost prompt's history walk is in
    /// its buffer, as `M-p` and `M-n` do. Past the newest, what was typed comes back.
    ///
    /// # Errors
    ///
    /// [`PromptError::NoHistory`] with no history to walk, and [`PromptError::Invalid`] at
    /// its end. Nothing changes.
    pub fn walk_history(&mut self, older: bool) -> Result<(), PromptError> {
        let Some(prompt) = self.prompts.last() else {
            return Err(PromptError::NoPrompt);
        };
        let name = prompt
            .kind
            .history()
            .filter(|_| !prompt.around_secret)
            .ok_or(PromptError::NoHistory)?;
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
        // What was typed is kept once, as the walk begins, and given back as it ends.
        let typed = match prompt.walk {
            None => self.buffer(id).map(|buffer| buffer.text().to_string()),
            Some(_) => None,
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
            if typed.is_some() {
                prompt.typed = typed;
            } else if next.is_none() {
                prompt.typed = None;
            }
        }
        Ok(())
    }

    /// Shows `text` in the innermost prompt until the next key, as a message from a key
    /// typed there does.
    pub fn prompt_says(&mut self, text: impl Into<String>) {
        self.say_in_prompt(text.into(), false);
    }

    /// Shows `text` in the innermost prompt, withheld from the control socket if `withheld`.
    pub(super) fn say_in_prompt(&mut self, text: String, withheld: bool) {
        if let Some(prompt) = self.prompts.last_mut() {
            prompt.message = Some(text);
            prompt.message_withheld = withheld;
        }
        self.relayout_prompt();
    }

    /// Where the innermost prompt's parts go, if one is open. It is worked out when what it
    /// depends on changes, so a frame or an event only reads it.
    pub fn prompt_layout(&self) -> Option<&PromptLayout> {
        self.prompt_layout.as_ref()
    }

    /// Works out where the innermost prompt's parts go, and gives its window the size of
    /// where its answer is typed. Called whenever the frame, the innermost prompt or its
    /// message changes.
    pub(super) fn relayout_prompt(&mut self) {
        self.prompt_layout = self.prompts.last().map(|prompt| {
            let message = prompt.message.as_deref();
            match prompt.candidates() {
                // A prompt that completes floats at the center (ADR 0076).
                Some(candidates) => minibuffer::completion_layout(
                    &self.frame,
                    &prompt.label,
                    message,
                    candidates.total(),
                    candidates.previews(),
                ),
                None => minibuffer::layout(&self.frame, &prompt.label, prompt.kind, message),
            }
        });
        if let Some(layout) = &self.prompt_layout {
            let size = ViewSize::new(1, layout.input.columns);
            self.windows[self.selected].set_size(size);
        }
        self.keep_selection_in_view();
    }

    /// Closes the innermost prompt, with its window and its buffer, and selects the window it
    /// came from again.
    fn close_innermost(&mut self) -> Option<Prompt> {
        // A passphrase's prompt is a buffer that may hold secrets going off the screen, so a
        // command running now says nothing more (ADR 0063).
        if self.prompts.last()?.kind == Kind::Passphrase {
            self.window_moves_on();
        }
        let prompt = self.prompts.pop()?;
        self.forget_marks(prompt.id);
        self.forget_files(prompt.id);
        self.windows.pop();
        self.selected = prompt.beneath;
        let id = prompt.buffer;
        self.buffers.retain(|(other, _)| *other != id);
        self.relayout_prompt();
        Some(prompt)
    }

    /// Does what one `C-g` does once a pending prefix is dealt with: the first that applies of
    /// the steps of ADR 0072: the region, then the innermost prompt, then isearch's two
    /// steps (ADR 0082). M2-21 (extra cursors) adds its part after them. Closing a question runs nothing and forgets
    /// a refused exit.
    pub fn quit(&mut self) -> Quit {
        // A region in a prompt goes before the prompt, as with Emacs's
        // `delete-selection-mode`.
        if self.deactivate_mark() {
            return Quit::Region;
        }
        if let Some(prompt) = self.prompts.last() {
            if prompt.kind.is_question() {
                self.reset_exit_guard();
            }
            self.close_innermost();
            return Quit::Prompt;
        }
        if let Some(quit) = self.isearch_quit() {
            return quit;
        }
        Quit::Nothing
    }

    /// Clears what a key typed in the innermost prompt said, as the next key does.
    pub(super) fn clear_prompt_message(&mut self) {
        if let Some(prompt) = self.prompts.last_mut()
            && prompt.message.take().is_some()
        {
            self.relayout_prompt();
        }
    }

    /// Marks that the command about to run gets a passphrase: what it says stays out of the
    /// log and `*Messages*`, as around a buffer that may hold secrets, and its echo out of
    /// the control socket, until the next key (ADR 0072). The dispatcher calls this after
    /// [`Editor::begin_command`].
    pub fn command_reads_passphrase(&mut self) {
        self.passphrase_command = self.in_command();
    }

    /// Whether a message said now goes in the innermost prompt: one from a command run while
    /// it is selected.
    pub(super) fn message_goes_in_prompt(&self) -> bool {
        self.in_command() && !self.prompts.is_empty()
    }
}
