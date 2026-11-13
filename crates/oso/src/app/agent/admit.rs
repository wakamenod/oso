//! Starting and stopping the agent channel at the person's command, or at launch as the
//! configuration file says, and asking the person about each connection once they are free
//! (ADR 0085, ADR 0108).
//!
//! Must not depend on: sockets or threads, which stay behind the channel's requests.

use std::time::Instant;

use serde_json::Value;

use oso_core::editor::Editor;
use oso_platform::control::PeerProcess;

use super::super::{App, UserEvent};
use super::AGENT_VERBS;
use crate::channel;

/// Whether the question about a connection waits: for a prompt to close, for keys `typing`
/// or a composition, for a jump (ADR 0116), whose keys it would take, for a keyboard
/// macro, as a replay never answers it and a recording would keep the answer, and while `!` or
/// an undo holds the keys, which it would abandon (ADR 0119).
fn question_waits(editor: &Editor, typing: bool) -> bool {
    editor.prompt().is_some()
        || typing
        || editor.jump().is_some()
        || editor.macro_recording()
        || editor.macro_replaying()
        || crate::dispatch::holds_input(editor)
}

impl App {
    /// Starts the channel at launch, as the configuration file called `file` says
    /// (ADR 0108). Each connection still asks the person.
    pub(in crate::app) fn start_agent_channel_as_configured(&mut self, file: &str) {
        self.start_agent_channel();
        if self.agent.starting {
            self.agent.started_by = Some(file.to_owned());
        }
    }

    /// Starts the channel on a thread of its own, as it checks and binds in the file system.
    pub(super) fn start_agent_channel(&mut self) {
        self.agent.stop_when_started = false;
        if self.agent.channel.is_some() || self.agent.starting {
            // Asked for by the person or the configuration: no refused session start turns
            // it off any more.
            self.agent.on_for_sessions = false;
            self.agent.started_for_a_session = false;
            self.editor.message("The agent channel is on already");
            return;
        }
        self.agent.started_by = None;
        self.agent.started_for_a_session = false;
        self.agent.on_for_sessions = false;
        self.agent.run += 1;
        let run = self.agent.run;
        let waker = self.waker.clone();
        let spawned = std::thread::Builder::new()
            .name("oso-agent-start".into())
            .spawn(move || {
                let send = waker.clone();
                let started =
                    channel::start(run, move |request| send.send(UserEvent::Agent(request)));
                waker.send(UserEvent::AgentStarted(started));
            });
        match spawned {
            Ok(_) => self.agent.starting = true,
            Err(e) => self
                .editor
                .message(format!("The agent channel did not start: {e}")),
        }
    }

    /// The channel's thread started it, or could not.
    pub(in crate::app) fn agent_started(&mut self, started: std::io::Result<channel::Channel>) {
        self.agent.starting = false;
        let started_by = self.agent.started_by.take();
        let for_a_session = std::mem::take(&mut self.agent.started_for_a_session);
        if std::mem::take(&mut self.agent.stop_when_started) {
            return;
        }
        match started {
            Ok(channel) => {
                self.agent.channel = Some(channel);
                self.agent.started = Some(Instant::now());
                self.agent_log("the agent channel is on");
                if for_a_session {
                    self.agent.on_for_sessions = true;
                    self.editor.message(
                        "Agent channel on for the session's oso tools: any other agent asks you first",
                    );
                    return;
                }
                let on = match started_by {
                    Some(file) => format!("Agent channel on, as {file} says"),
                    None => "Agent channel on".to_owned(),
                };
                self.editor
                    .message(format!("{on}: each agent asks you first"));
            }
            Err(e) => {
                log::warn!("the agent channel did not start: {e}");
                self.editor
                    .message(format!("The agent channel did not start: {e}"));
            }
        }
    }

    pub(super) fn stop_agent_channel(&mut self) {
        self.agent.on_for_sessions = false;
        if self.agent.starting {
            self.agent.stop_when_started = true;
            self.editor.message("Agent channel off");
            return;
        }
        let Some(channel) = self.agent.channel.take() else {
            self.editor.message("The agent channel is off already");
            return;
        };
        drop(channel);
        if let Some(asking) = self.agent.asking.take() {
            let _ = asking
                .reply
                .send(Err("the person turned the agent channel off".into()));
            if let Some(question) = asking.question
                && self
                    .editor
                    .prompt()
                    .is_some_and(|prompt| prompt.id() == question)
            {
                self.editor.quit();
            }
        }
        self.agent.connected.clear();
        self.session_tools_gone(None);
        self.session_bridge_gone(None);
        self.forget_undo_of(None);
        self.orphan_questions(None);
        self.drop_agent_opens(None);
        self.agent.opened.clear();
        self.agent_log("the agent channel is off");
        self.editor.message("Agent channel off");
    }

    /// Whether a connection waits for the person's answer, or to be asked.
    pub(in crate::app) fn agent_asks(&self) -> bool {
        self.agent.asking.is_some()
    }

    /// Whether a question the agent causes may open now: no connection waits to be asked
    /// about, and the person is free as the connection's question needs them to be.
    pub(in crate::app) fn agent_question_may_open(&self) -> bool {
        !self.agent_asks() && !self.person_busy()
    }

    /// Whether a question an agent causes must wait: a prompt is open, keys or a jump are under
    /// way, a keyboard macro is recorded or replayed, a menu is open (ADR 0117) or the input
    /// method composes.
    fn person_busy(&self) -> bool {
        question_waits(
            &self.editor,
            self.dispatcher.keys_in_progress() || self.dispatcher.menu_open() || self.composing(),
        )
    }

    /// A question the agent caused just opened: it takes no answer until a quiet moment.
    pub(in crate::app) fn note_agent_question_opened(&mut self) {
        self.agent.quiet_since = Some(Instant::now());
    }

    /// The question `id` the agent caused lost its connection: it closes once innermost, and
    /// no `y` answers it any more.
    pub(in crate::app) fn orphan_agent_question(&mut self, id: oso_core::minibuffer::PromptId) {
        self.agent.orphaned.push(id);
    }

    /// Asks about the waiting connection once the person is not busy, and takes their answer
    /// once its question has closed.
    pub(super) fn ask_about_a_connection(&mut self) {
        let Some(asking) = &self.agent.asking else {
            return;
        };
        match asking.question {
            None => {
                if self.person_busy() {
                    return;
                }
                let label = channel::question::connection(&asking.peer);
                let Ok(id) = self
                    .editor
                    .ask_for_agent(label, "agent-channel-allow", AGENT_VERBS)
                else {
                    return;
                };
                self.agent.quiet_since = Some(Instant::now());
                if let Some(asking) = &mut self.agent.asking {
                    asking.question = Some(id);
                }
                self.request_redraw();
            }
            Some(id) => {
                if self.editor.prompts().iter().any(|prompt| prompt.id() == id) {
                    return;
                }
                let Some(asking) = self.agent.asking.take() else {
                    return;
                };
                let pid = asking.peer.process.as_ref().map(PeerProcess::pid);
                let who = pid.map_or_else(|| "an agent".to_owned(), |pid| format!("pid {pid}"));
                if self.editor.take_agent_allowed() {
                    let _ = asking.reply.send(Ok(Value::Null));
                    self.agent.connected.push(asking.connection);
                    self.agent_log(&format!("{who}: let in"));
                } else {
                    let _ = asking
                        .reply
                        .send(Err(channel::REFUSED_BY_THE_PERSON.into()));
                    self.agent_log(&format!("{who}: refused"));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use oso_core::buffer::Buffer;
    use oso_core::editor::{Editor, MacroEvent};
    use oso_core::jump::Kind;

    use super::question_waits;

    #[test]
    fn the_question_about_a_connection_waits_for_a_keyboard_macro() {
        let mut editor = Editor::with_buffer(Buffer::from_text("notes.txt", ""));
        assert!(!question_waits(&editor, false));
        assert!(question_waits(&editor, true));
        editor.macro_start().unwrap();
        assert!(question_waits(&editor, false), "while recording");
        editor.macro_record([MacroEvent::Text {
            text: "y".into(),
            commit: false,
        }]);
        editor.macro_end().unwrap();
        assert!(!question_waits(&editor, false));
        editor.macro_call(None).unwrap();
        assert!(question_waits(&editor, false), "while replaying");
        editor.macro_stop(None);
        assert!(!question_waits(&editor, false));
    }

    #[test]
    fn the_question_about_a_connection_waits_for_an_undo_under_way() {
        let mut editor = Editor::with_buffer(Buffer::from_text("notes.txt", ""));
        crate::channel::state::begin_a_large_undo(&mut editor);
        assert!(question_waits(&editor, false));
        editor.cancel_crossing();
        assert!(!question_waits(&editor, false));
    }

    #[test]
    fn the_question_about_a_connection_waits_for_a_jump() {
        let mut editor = Editor::with_buffer(Buffer::from_text("notes.txt", "ab ab"));
        assert!(editor.jump_begin(Kind::Char, false));
        assert!(question_waits(&editor, false));
        editor.quit();
        assert!(editor.jump().is_none());
        assert!(!question_waits(&editor, false));
    }
}
