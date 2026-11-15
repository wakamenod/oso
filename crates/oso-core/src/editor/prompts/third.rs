//! A question's third answer (ADR 0133): asking a question that has one, on `w`, and giving
//! it, which leaves its command to run as `y` leaves its own.
//!
//! Must not depend on: `command` or `keymap`; the dispatcher runs what the answer names.

use super::super::Editor;
use crate::minibuffer::{Kind, Outcome, Prompt, PromptError, PromptId, Third};

impl Editor {
    /// Asks `question` as [`Self::ask_with`] does, with a third answer on `w`, which runs
    /// `third.command`, confirmed, as `y` runs `yes` (ADR 0133).
    ///
    /// # Errors
    ///
    /// [`PromptError::TooDeep`] with [`MAX_DEPTH`](crate::minibuffer::MAX_DEPTH) open already.
    pub fn ask_with_third(
        &mut self,
        question: impl Into<String>,
        yes: &'static str,
        verbs: [&'static str; 2],
        third: Third,
    ) -> Result<PromptId, PromptError> {
        let third = Some(third);
        self.open_prompt(question, Kind::Question { yes, verbs, third })
    }

    /// Answers the innermost prompt's question with its third answer and closes it, leaving
    /// that answer's command for [`Editor::take_prompt_outcome`], as `y` does (ADR 0133).
    ///
    /// # Errors
    ///
    /// [`PromptError::NoPrompt`] unless the innermost prompt is a question with a third
    /// answer.
    pub fn answer_question_third(&mut self) -> Result<(), PromptError> {
        let Some(Kind::Question {
            third: Some(third), ..
        }) = self.prompt().map(Prompt::kind)
        else {
            return Err(PromptError::NoPrompt);
        };
        let agents = self.prompt().is_some_and(Prompt::from_agent);
        self.close_innermost();
        self.note_agents_answer(agents);
        self.outcome = Some(Outcome::Run(third.command));
        Ok(())
    }
}
