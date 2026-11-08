//! What the editor knows of the agent channel (ADR 0085): whether it is on and whether an
//! agent is connected, for the modeline; what a person asked of it, for the host; the
//! prompts the channel caused, labelled for the person; and `*Agent*`, the list of what was
//! asked over it.
//!
//! Must not depend on: sockets, threads or the host's channel; the host says what holds, and
//! takes what was asked.

use super::Editor;
use crate::buffer::Buffer;
use crate::minibuffer::PromptId;

/// The name of the internal buffer that lists the channel's requests.
pub const AGENT_LOG: &str = "*Agent*";

/// What a label of a prompt the channel caused starts with, so that the person can tell it
/// from oso's own.
pub const AGENT_LABEL: &str = "Agent: ";

/// The most rows `*Agent*` keeps; the oldest goes first.
pub const MAX_AGENT_LOG: usize = 1000;

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
    log: Option<crate::buffer::BufferId>,
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

    /// Marks the prompt `id` as one the agent channel caused: its label starts with
    /// [`AGENT_LABEL`], after its depth. Says whether it is open.
    pub fn mark_from_agent(&mut self, id: PromptId) -> bool {
        let Some(prompt) = self.prompts.iter_mut().find(|prompt| prompt.id == id) else {
            return false;
        };
        if !prompt.from_agent {
            prompt.from_agent = true;
            let at = match prompt.label.strip_prefix('[') {
                Some(rest) => rest.find("] ").map_or(0, |end| end + 3),
                None => 0,
            };
            prompt.label.insert_str(at, AGENT_LABEL);
            self.relayout_prompt();
        }
        true
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
    pub fn agent_log_buffer(&self) -> Option<crate::buffer::BufferId> {
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
        let first = editor.ask("Save over notes.md?", "save-buffer").unwrap();
        assert!(editor.mark_from_agent(first));
        let prompt = editor.prompt().unwrap();
        assert!(prompt.from_agent());
        assert_eq!(prompt.label(), "Agent: Save over notes.md? (y or n)");
        // Marking twice changes nothing.
        assert!(editor.mark_from_agent(first));
        assert_eq!(
            editor.prompt().unwrap().label(),
            "Agent: Save over notes.md? (y or n)"
        );

        let second = editor.ask("Exit?", "save-buffers-kill-terminal").unwrap();
        assert!(!editor.prompt().unwrap().from_agent());
        editor.mark_from_agent(second);
        assert_eq!(
            editor.prompt().unwrap().label(),
            "[2] Agent: Exit? (y or n)"
        );
    }

    #[test]
    fn a_closed_prompt_is_not_marked() {
        let mut editor = Editor::default();
        let id = editor.ask("Exit?", "save-buffers-kill-terminal").unwrap();
        editor.quit();
        assert!(!editor.mark_from_agent(id));
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
