//! What the editor knows of the agent channel (ADR 0085): whether it is on and whether an
//! agent is connected, for the modeline; what a person asked of it, for the host; the
//! prompts the channel caused, labelled for the person; and `*Agent*`, the list of what was
//! asked over it.
//!
//! Must not depend on: sockets, threads or the host's channel; the host says what holds, and
//! takes what was asked.

use super::Editor;
use crate::buffer::{Buffer, BufferId};
use crate::minibuffer::{PromptError, PromptId};

/// The name of the internal buffer that lists the channel's requests.
pub(crate) const AGENT_LOG: &str = "*Agent*";

/// What a label of a prompt the channel caused starts with, after its depth, so that the
/// person can tell it from oso's own.
pub(crate) const AGENT_LABEL: &str = "Agent: ";

/// The most rows `*Agent*` keeps; the oldest goes first.
pub(crate) const MAX_AGENT_LOG: usize = 1000;

/// Whether the agent channel is on, for the modeline.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AgentChannel {
    /// Off, as it starts.
    #[default]
    Off,
    /// On, with no agent connected.
    On,
    /// An agent the person let in is connected.
    Connected,
}

/// The editor's part of the agent channel.
#[derive(Debug, Default)]
pub(super) struct Agent {
    status: AgentChannel,
    /// The person ran a command to turn it on or off, for the host.
    request: Option<bool>,
    /// The person answered `y` to a connection's question, for the host.
    allowed: bool,
    /// A command runs for the channel now: the prompts it opens are the agent's.
    running: bool,
    /// The person just answered `y` to a question the agent caused, so what the answer runs
    /// is the agent's too.
    answered: bool,
    log: Option<BufferId>,
}

impl Editor {
    /// Whether the agent channel is on, and whether an agent is connected.
    pub fn agent_channel(&self) -> AgentChannel {
        self.agent.status
    }

    /// The host says whether the channel is on and an agent connected.
    pub fn set_agent_channel(&mut self, status: AgentChannel) {
        self.agent.status = status;
    }

    /// A command asks the host to turn the channel on or off.
    pub fn request_agent_channel(&mut self, on: bool) {
        self.agent.request = Some(on);
    }

    /// What a command asked of the channel since the host last looked.
    pub fn take_agent_channel_request(&mut self) -> Option<bool> {
        self.agent.request.take()
    }

    /// The person let the waiting connection in, by answering its question.
    pub fn allow_agent(&mut self) {
        self.agent.allowed = true;
    }

    /// Whether the person let the waiting connection in since the host last looked.
    pub fn take_agent_allowed(&mut self) -> bool {
        std::mem::take(&mut self.agent.allowed)
    }

    /// Runs `work` as the agent channel's: each prompt it opens is the agent's, and its label
    /// starts with `Agent: `, after its depth (ADR 0085).
    pub fn for_agent<R>(&mut self, work: impl FnOnce(&mut Self) -> R) -> R {
        let outer = std::mem::replace(&mut self.agent.running, true);
        let out = work(self);
        self.agent.running = outer;
        out
    }

    /// Whether what runs now runs for the agent channel: a command it ran, or the command
    /// the person's `y` to its question runs.
    pub fn runs_for_agent(&self) -> bool {
        self.agent.running
    }

    /// The question just answered `y` was the agent's.
    pub(super) fn note_agents_answer(&mut self, agents: bool) {
        self.agent.answered = agents;
    }

    /// Whether the question just answered `y` was the agent's, once, for the dispatcher to
    /// run its command as the agent's.
    pub fn take_agents_answer(&mut self) -> bool {
        std::mem::take(&mut self.agent.answered)
    }

    /// Asks `question` for the agent channel, as [`Self::ask_with`] does, as a prompt that is
    /// the agent's. Forgets an earlier `y` first, so that only this question's answer counts.
    ///
    /// # Errors
    ///
    /// As [`Self::ask_with`].
    pub fn ask_for_agent(
        &mut self,
        question: impl Into<String>,
        yes: &'static str,
        verbs: [&'static str; 2],
    ) -> Result<PromptId, PromptError> {
        self.agent.allowed = false;
        self.for_agent(|editor| editor.ask_with(question, yes, verbs))
    }

    /// Adds a row to `*Agent*`, which is made the first time. `row` is one line, which the
    /// caller builds from what it may show: never a text argument or buffer text.
    pub fn agent_log(&mut self, row: &str) {
        let id = match self.agent.log {
            Some(id) if self.buffer(id).is_some() => id,
            _ => {
                let id = self.add_buffer(Buffer::internal(AGENT_LOG));
                self.agent.log = Some(id);
                id
            }
        };
        let row: String = row
            .chars()
            .map(|c| if c == '\n' { ' ' } else { c })
            .collect();
        self.add_row(id, &row, MAX_AGENT_LOG);
    }

    /// The buffer `*Agent*`, once something was logged there.
    pub(super) fn agent_log_buffer(&self) -> Option<BufferId> {
        self.agent.log
    }
}

#[cfg(test)]
mod tests {
    use super::{AGENT_LOG, AgentChannel, MAX_AGENT_LOG};
    use crate::editor::Editor;

    #[test]
    fn a_prompt_the_agent_caused_says_so_after_its_depth() {
        let mut editor = Editor::default();
        editor.for_agent(|editor| editor.ask("Save over notes.md?", "save-buffer").unwrap());
        let prompt = editor.prompt().unwrap();
        assert!(prompt.from_agent());
        assert_eq!(prompt.label(), "Agent: Save over notes.md? (y or n)");

        editor.ask("Exit?", "save-buffers-kill-terminal").unwrap();
        assert!(!editor.prompt().unwrap().from_agent(), "a person's own");
        editor
            .ask_for_agent("Let it in?", "agent-channel-allow", ["allow", "refuse"])
            .unwrap();
        assert_eq!(
            editor.prompt().unwrap().label(),
            "[3] Agent: Let it in? (y or n)"
        );
    }

    #[test]
    fn asking_for_the_agent_forgets_an_earlier_yes() {
        let mut editor = Editor::default();
        editor.allow_agent();
        editor
            .ask_for_agent("Let it in?", "agent-channel-allow", ["allow", "refuse"])
            .unwrap();
        assert!(!editor.take_agent_allowed());
    }

    #[test]
    fn requests_and_answers_are_taken_once() {
        let mut editor = Editor::default();
        assert_eq!(editor.agent_channel(), AgentChannel::Off);
        editor.request_agent_channel(true);
        assert_eq!(editor.take_agent_channel_request(), Some(true));
        assert_eq!(editor.take_agent_channel_request(), None);
        editor.allow_agent();
        assert!(editor.take_agent_allowed());
        assert!(!editor.take_agent_allowed());
        editor.set_agent_channel(AgentChannel::Connected);
        assert_eq!(editor.agent_channel(), AgentChannel::Connected);
    }

    #[test]
    fn the_log_keeps_one_row_per_request_and_drops_the_oldest() {
        let mut editor = Editor::default();
        assert_eq!(editor.agent_log_buffer(), None);
        for n in 0..=MAX_AGENT_LOG {
            editor.agent_log(&format!("run forward-char {n}\nok"));
        }
        let id = editor.agent_log_buffer().unwrap();
        let buffer = editor.buffer(id).unwrap();
        assert_eq!(buffer.name(), AGENT_LOG);
        let text = buffer.text().to_string();
        assert_eq!(text.lines().count(), MAX_AGENT_LOG);
        assert!(text.starts_with("run forward-char 1 ok\n"), "{text:.40}");
        assert!(text.ends_with(&format!("run forward-char {MAX_AGENT_LOG} ok\n")));
    }
}
