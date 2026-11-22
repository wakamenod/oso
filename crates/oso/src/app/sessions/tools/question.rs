//! A session tool's question to the person (ADR 0121, step 6): put once the person is free,
//! answered from the window only, `?` showing what it would run, and closed when its connection
//! goes. `y` carries out its plan as the person's act; `n` or `C-g` refuses the same call from
//! the same session for a minute.
//!
//! Must not depend on: key presses or how a frame is drawn.

use std::time::Instant;

use oso_core::buffer::{Buffer, Mode};
use oso_core::minibuffer::PromptId;

use super::super::super::App;
use super::Pending;
use super::asks::Plan;
use crate::sessions::identity::SessionId;
use crate::sessions::tools as rules;

/// The name of the buffer `?` shows.
const DETAILS: &str = "*Agent request*";
/// What the agent is told when the person says no.
const REFUSED: &str =
    "the person refused it; that is their decision: do not reach the same effect another way";

/// A tool's question, before and while it is asked.
#[derive(Debug)]
pub(in crate::app) struct Question {
    pub(super) pending: Pending,
    pub(super) caller: SessionId,
    /// What the refusal is kept for: the branch, or the directory.
    pub(super) target: String,
    pub(super) label: String,
    /// What `?` shows.
    pub(super) details: String,
    pub(super) plan: Plan,
    pub(super) asked: Option<PromptId>,
}

impl App {
    /// Puts `plan`'s question `label`, with `details` behind `?`, to the person once they are
    /// free: `caller`'s call on `target`, whose `n` refuses the same call for a minute.
    pub(super) fn queue_question(
        &mut self,
        pending: Pending,
        (caller, target): (SessionId, String),
        (label, details): (String, String),
        plan: Plan,
    ) {
        self.hosting.tools.question = Some(Question {
            pending,
            caller,
            target,
            label,
            details,
            plan,
            asked: None,
        });
        self.ask_about_a_tool();
    }

    /// Asks the waiting tool's question once the person is free, and takes the answer once it
    /// has closed.
    pub(super) fn ask_about_a_tool(&mut self) {
        let Some(question) = &self.hosting.tools.question else {
            return;
        };
        match question.asked {
            None => {
                if !self.agent_question_may_open() {
                    return;
                }
                let label = question.label.clone();
                match self.editor.ask_for_agent(
                    label,
                    "agent-channel-allow",
                    crate::app::agent::AGENT_VERBS,
                ) {
                    Ok(id) => {
                        if let Some(question) = &mut self.hosting.tools.question {
                            question.asked = Some(id);
                        }
                        self.note_agent_question_opened();
                        self.request_redraw();
                    }
                    Err(e) => {
                        if let Some(question) = self.hosting.tools.question.take() {
                            question.pending.answer(Err(e.to_string()));
                        }
                    }
                }
            }
            Some(id) => {
                if self.editor.prompts().iter().any(|prompt| prompt.id() == id) {
                    return;
                }
                let Some(question) = self.hosting.tools.question.take() else {
                    return;
                };
                self.hide_question_details();
                if self.editor.take_agent_allowed() {
                    self.agent_log(&format!("{}: allowed", question.pending.tool));
                    self.carry_out(question.pending, question.plan, None);
                } else {
                    let tool = question.pending.tool;
                    self.hosting.tools.guard.refused(
                        &question.caller,
                        tool,
                        &question.target,
                        Instant::now(),
                    );
                    self.agent_log(&format!("{tool}: refused"));
                    question.pending.answer(Err(REFUSED.into()));
                }
            }
        }
    }

    /// The open question of a session tool, if one is asked.
    pub(in crate::app) fn session_tool_question(&self) -> Option<PromptId> {
        self.hosting.tools.question.as_ref().and_then(|q| q.asked)
    }

    /// The question's connection went: it closes, and nothing it asked for runs.
    pub(super) fn close_tool_question(&mut self) {
        let Some(question) = self.hosting.tools.question.take() else {
            return;
        };
        self.hide_question_details();
        if let Some(id) = question.asked {
            self.orphan_agent_question(id);
        }
    }

    /// `?` in the tool's open question: `*Agent request*` shows what it would run; in a native
    /// session's permission question, `*Session call*` the call (ADR 0165, step 2).
    pub(in crate::app) fn show_question_details(&mut self) {
        if self.show_permission_details() {
            return;
        }
        let Some(question) = self
            .hosting
            .tools
            .question
            .as_ref()
            .filter(|question| question.asked.is_some())
        else {
            return self
                .editor
                .message("This question has nothing more to show");
        };
        let text = question.details.clone();
        let kept = self.hosting.tools.details;
        let id = match kept.filter(|&id| self.editor.buffer(id).is_some_and(Buffer::is_internal)) {
            Some(id) => id,
            None => self.editor.add_internal_buffer(DETAILS, Mode::Fundamental),
        };
        self.hosting.tools.details = Some(id);
        // The agent's own request: no agent reads it back, as none reads `*Brief*`.
        if let Some(buffer) = self.editor.buffer_mut(id) {
            buffer.withhold_from_agents();
        }
        self.editor.set_internal_text(id, &text);
        self.editor.show(id);
        self.request_redraw();
    }

    fn hide_question_details(&mut self) {
        if let Some(id) = self.hosting.tools.details.take()
            && self.editor.buffer(id).is_some()
        {
            self.editor.kill_buffer(id);
        }
    }
}

/// What `?` shows: who asks, the question, and `more`.
pub(super) fn details(caller: &str, label: &str, more: &str) -> String {
    format!(
        "Session {} asks:\n\n  {label}\n\ny allows it, n or C-g refuses it.\n\n{more}\n",
        rules::quoted(caller)
    )
}
