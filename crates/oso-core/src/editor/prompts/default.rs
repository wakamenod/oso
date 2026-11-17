//! A prompt's default, which `M-n` gives from what was typed, as Emacs's future history does
//! (ADR 0138, step 14): the host offers it once the prompt is open, such as a rename's old name.
//!
//! Must not depend on: `command` or `keymap`; `M-n` and `M-p` reach it through `walk_history`.
//!
//! - `M-n` from what was typed shows the default, and `M-p` from it gives what was typed back;
//!   past it, `M-n` is at the end. A walk of the history goes on from what was typed, as before.
//! - It is never an answer by itself: an empty `RET` stays empty.

use crate::buffer::Replacement;
use crate::minibuffer::{PromptError, PromptId};

use super::super::Editor;

impl Editor {
    /// Offers `text` as the default of the open prompt `prompt`, which `M-n` gives from what
    /// was typed. Nothing changes for a prompt not open, or one whose buffer may hold secrets.
    pub fn offer_prompt_default(&mut self, prompt: PromptId, text: impl Into<String>) {
        let text: String = text.into();
        if let Some(open) = (self.prompts.iter_mut()).find(|open| open.id == prompt)
            && !matches!(open.kind, crate::minibuffer::Kind::Passphrase)
        {
            open.default = Some((text.into(), false));
        }
    }

    /// The innermost prompt's walk to or from its default, as `M-n` or (`older`) `M-p` makes it;
    /// `None` when the walk is the history's.
    pub(super) fn walk_default(&mut self, older: bool) -> Option<Result<(), PromptError>> {
        let prompt = self.prompts.last()?;
        let (default, shown) = prompt.default.as_ref()?;
        let text = match (*shown, older, prompt.walk) {
            // From what was typed, to the default.
            (false, false, None) => default.to_string(),
            // From the default, back to what was typed.
            (true, true, _) => prompt.typed.clone().unwrap_or_default(),
            (true, false, _) => return Some(Err(PromptError::Invalid)),
            _ => return None,
        };
        let id = prompt.buffer;
        let typed = (!*shown).then(|| self.buffer(id).map(|buffer| buffer.text().to_string()));
        let end = self.buffer(id).map_or(0, |buffer| buffer.text().len());
        let edit = Replacement {
            range: 0..end,
            text: &text,
        };
        if self.apply_edits(id, &[edit]).is_err() {
            return Some(Err(PromptError::NoPrompt));
        }
        let prompt = self.prompts.last_mut()?;
        if let Some((_, shown)) = prompt.default.as_mut() {
            *shown = !*shown;
        }
        match typed {
            Some(typed) => prompt.typed = typed,
            None => prompt.typed = None,
        }
        Some(Ok(()))
    }
}

#[cfg(test)]
mod tests;
