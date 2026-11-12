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
use super::CONNECTION_VERBS;
use crate::channel;

/// Whether the question about a connection waits: for a prompt to close, for keys `typing`
/// or a composition, and for a keyboard macro, as a replay never answers it and a recording
/// would keep the answer.
fn question_waits(editor: &Editor, typing: bool) -> bool {
    editor.prompt().is_some() || typing || editor.macro_recording() || editor.macro_replaying()
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
            self.editor.message("The agent channel is on already");
            return;
        }
        self.agent.started_by = None;
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
        if std::mem::take(&mut self.agent.stop_when_started) {
            return;
        }
        match started {
            Ok(channel) => {
                self.agent.channel = Some(channel);
                self.agent.started = Some(Instant::now());
                self.agent_log("the agent channel is on");
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
        self.orphan_questions(None);
        self.drop_agent_opens(None);
        self.agent.opened.clear();
        self.agent_log("the agent channel is off");
        self.editor.message("Agent channel off");
    }

    /// Asks about the waiting connection once no prompt is open, no keys are in progress and
    /// no keyboard macro is recorded or replayed, and takes the person's answer once its question has closed.
    pub(super) fn ask_about_a_connection(&mut self) {
        let Some(asking) = &self.agent.asking else {
            return;
        };
        match asking.question {
            None => {
                if question_waits(
                    &self.editor,
                    self.dispatcher.keys_in_progress() || self.composing(),
                ) {
                    return;
                }
                let label = channel::question::connection(&asking.peer);
                let Ok(id) =
                    self.editor
                        .ask_for_agent(label, "agent-channel-allow", CONNECTION_VERBS)
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
}
