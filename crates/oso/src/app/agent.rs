//! The agent channel on the UI thread (ADR 0085): starting and stopping it at the person's
//! command, asking the person about each connection, and answering the requests of the
//! connections let in, while the person is not in the middle of something.
//!
//! Must not depend on: sockets or threads, which stay behind the channel's requests, or
//! winit.
//!
//! - Nothing runs while a buffer that may hold secrets is shown, while a prompt the person
//!   opened, keys in progress or a composition is open, within [`PERSON_TYPING`] of the
//!   person's last key, or while a question the channel caused is open.
//! - A question the channel caused, the connection's own included, says `Agent:` first, is
//!   answered only from the window, and takes no answer until [`QUIET`] has passed with no
//!   key.
//! - What the channel's commands kill stays in oso's kill ring.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use oso_core::command::Reach;
use oso_core::editor::AgentChannel;
use oso_core::key::KeyPress;
use oso_core::minibuffer::PromptId;
use oso_platform::control::{Peer, PeerProcess};

use super::App;
use super::UserEvent;
use super::shell::Loop;
use crate::channel::{self, Connection, Event, Reply, protocol::Op};

/// How long a question the channel caused takes no answer after the last key.
pub const QUIET: Duration = Duration::from_millis(500);

/// How long after the person's last key the channel runs nothing.
pub const PERSON_TYPING: Duration = Duration::from_secs(1);

/// The verbs of the connection's question, after `y` and `n`.
const CONNECTION_VERBS: [&str; 2] = ["allow", "refuse"];

/// The agent channel's part of the app.
#[derive(Debug, Default)]
pub(super) struct Agent {
    channel: Option<channel::Channel>,
    started: Option<Instant>,
    /// The connections the person let in.
    connected: Vec<Connection>,
    /// The connection that waits for the person, and its question once it is asked.
    asking: Option<Asking>,
    /// The questions the channel's commands caused.
    caused: Vec<PromptId>,
    /// When a key last came while a question the channel caused was innermost, or when it
    /// opened.
    quiet_since: Option<Instant>,
    /// When the person last typed in the window.
    person_typed: Option<Instant>,
}

#[derive(Debug)]
struct Asking {
    connection: Connection,
    peer: Peer,
    reply: std::sync::mpsc::Sender<Reply>,
    question: Option<PromptId>,
}

/// What the connection's question says: who reports itself as connecting, as the system
/// names them. `oso mcp` is named so only when the process runs this program; its parent
/// counts only then, and not when it is the system's first process, which adopts orphans.
pub fn connection_question(peer: &Peer) -> String {
    let named = |process: &PeerProcess| match process.name() {
        Some(name) => format!("\"{name}\" (pid {})", process.pid()),
        None => format!("an unnamed process (pid {})", process.pid()),
    };
    match (&peer.process, peer.runs_this_program) {
        (Some(process), true) => {
            let parent = peer
                .parent
                .as_ref()
                .filter(|parent| parent.pid() > 1)
                .map_or_else(|| "an unknown process".to_owned(), named);
            format!(
                "let a process that reports itself as {parent} control oso through oso mcp (pid {})?",
                process.pid()
            )
        }
        (Some(process), false) => {
            format!(
                "let a process that reports itself as {} control oso?",
                named(process)
            )
        }
        (None, _) => "let a process the system does not name control oso?".to_owned(),
    }
}

impl App {
    /// A connection of the agent channel brought something.
    pub(super) fn handle_agent(&mut self, event_loop: &Loop<'_>, request: channel::Request) {
        let channel::Request {
            connection,
            event,
            reply,
        } = request;
        if self.is_leaving(event_loop) {
            if let Some(reply) = reply {
                let _ = reply.send(Err("the editor is shutting down".into()));
            }
            return;
        }
        match event {
            Event::Asks(peer) => {
                let Some(reply) = reply else { return };
                if self.agent.asking.is_some() {
                    let _ = reply.send(Err("another agent waits for the person's answer".into()));
                    return;
                }
                self.agent.asking = Some(Asking {
                    connection,
                    peer,
                    reply,
                    question: None,
                });
            }
            Event::Gone => self.agent_gone(connection),
            Event::Op(op) => {
                let Some(reply) = reply else { return };
                let result = if self.agent.connected.contains(&connection) {
                    self.agent_op(event_loop, op)
                } else {
                    Err("the person has not let this connection in".into())
                };
                let _ = reply.send(result);
            }
        }
    }

    /// A connection closed, or the channel stopped under it.
    fn agent_gone(&mut self, connection: Connection) {
        self.agent.connected.retain(|&open| open != connection);
        if self
            .agent
            .asking
            .as_ref()
            .is_some_and(|asking| asking.connection == connection)
        {
            let asking = self.agent.asking.take();
            if let Some(question) = asking.and_then(|asking| asking.question)
                && self
                    .editor
                    .prompt()
                    .is_some_and(|prompt| prompt.id() == question)
            {
                self.editor.quit();
            }
            self.agent_log("an agent left before the person answered");
        } else {
            self.agent_log("an agent left");
        }
        self.request_redraw();
    }

    /// Answers a request of a connection the person let in.
    fn agent_op(&mut self, event_loop: &Loop<'_>, op: Op) -> Reply {
        match op {
            Op::State => Ok(channel::state::state(&self.editor)),
            Op::Commands => Ok(channel::state::commands(self.dispatcher.commands())),
            Op::Cancel => {
                let innermost = self.editor.prompt().map(|prompt| prompt.id());
                match innermost.filter(|id| self.agent.caused.contains(id)) {
                    Some(_) => {
                        self.editor.quit();
                        self.request_redraw();
                        self.agent_log("cancel: ok");
                        Ok(json!({ "cancelled": true }))
                    }
                    None => Err("no question the agent caused is open".into()),
                }
            }
            Op::Run { name, arg, reach } => self.agent_run(event_loop, &name, arg, reach),
        }
    }

    /// Runs a command for the channel, unless the person is in the middle of something.
    fn agent_run(
        &mut self,
        event_loop: &Loop<'_>,
        name: &str,
        arg: Option<isize>,
        reach: Reach,
    ) -> Reply {
        if let Some(why) = self.agent_waits() {
            return Err(why.into());
        }
        let newest = self.editor.prompts().last().map(|prompt| prompt.id());
        let ran = self
            .dispatcher
            .run_for_agent(&mut self.editor, name, arg, reach);
        let (effects, result) = match ran {
            Ok(ran) => ran,
            Err(refused) => {
                self.agent_log(&format!("run: refused: {}", refused.0));
                return Err(refused.0);
            }
        };
        // What the agent killed stays in oso's ring: never the OS clipboard (ADR 0085).
        drop(self.editor.take_clipboard_write());
        self.note_logging();
        let asked = self.note_caused(newest);
        self.apply(event_loop, effects);
        let shown_arg = arg.map_or_else(String::new, |n| format!(" {n}"));
        let outcome = match (&result, &asked) {
            (Err(message), _) => format!("failed: {message}"),
            (Ok(()), Some(_)) => "waiting for the person".to_owned(),
            (Ok(()), None) => "ok".to_owned(),
        };
        self.agent_log(&format!("run {name}{shown_arg}: {outcome}"));
        match (result, asked) {
            (Err(message), _) => Err(message),
            (Ok(()), Some(label)) => Ok(json!({
                "command": name,
                "waiting": format!("waiting for the person: {label}"),
            })),
            (Ok(()), None) => Ok(json!({ "command": name })),
        }
    }

    /// Why the channel may run nothing now, if it may not.
    fn agent_waits(&self) -> Option<&'static str> {
        if self.editor.shows_secret() {
            return Some("a buffer that may hold secrets is shown; ask the person");
        }
        let prompts = self.editor.prompts();
        if prompts.iter().any(|prompt| prompt.from_agent()) {
            return Some("a question the agent caused waits for the person");
        }
        if !prompts.is_empty() {
            return Some("the person is answering a prompt");
        }
        if self.dispatcher.reads_keys() {
            return Some("the person is in the middle of a key sequence");
        }
        if self.composing() {
            return Some("the person is composing text");
        }
        if self
            .agent
            .person_typed
            .is_some_and(|at| at.elapsed() < PERSON_TYPING)
        {
            return Some("the person is typing; try again in a moment");
        }
        None
    }

    /// Marks the prompts a channel command opened, those newer than `newest`, as caused by
    /// the agent, and returns the innermost one's label if one is a question.
    fn note_caused(&mut self, newest: Option<PromptId>) -> Option<String> {
        let opened: Vec<PromptId> = self
            .editor
            .prompts()
            .iter()
            .map(|prompt| prompt.id())
            .filter(|&id| newest.is_none_or(|newest| id > newest))
            .collect();
        for &id in &opened {
            self.editor.mark_from_agent(id);
            self.agent.caused.push(id);
            self.agent.quiet_since = Some(Instant::now());
        }
        opened.last()?;
        self.editor.prompt().map(|prompt| prompt.label().to_owned())
    }

    /// After each event: starts or stops the channel as the person asked, asks about a
    /// waiting connection once the person is free, takes the answer, and says in the
    /// modeline whether the channel is on.
    pub(super) fn sync_agent(&mut self) {
        match self.editor.take_agent_channel_request() {
            Some(true) => self.start_agent_channel(),
            Some(false) => self.stop_agent_channel(),
            None => {}
        }
        self.ask_about_a_connection();
        let open = self.editor.prompts();
        self.agent
            .caused
            .retain(|id| open.iter().any(|prompt| prompt.id() == *id));
        let status = match (&self.agent.channel, self.agent.connected.is_empty()) {
            (None, _) => AgentChannel::Off,
            (Some(_), true) => AgentChannel::On,
            (Some(_), false) => AgentChannel::Connected,
        };
        if self.editor.agent_channel() != status {
            self.editor.set_agent_channel(status);
            self.request_redraw();
        }
    }

    fn start_agent_channel(&mut self) {
        if self.agent.channel.is_some() {
            self.editor.message("The agent channel is on already");
            return;
        }
        let waker = self.waker.clone();
        match channel::start(move |request| waker.send(UserEvent::Agent(request))) {
            Ok(channel) => {
                self.agent.channel = Some(channel);
                self.agent.started = Some(Instant::now());
                self.agent_log("the agent channel is on");
                self.editor
                    .message("Agent channel on: each agent asks you first");
            }
            Err(e) => {
                log::warn!("the agent channel did not start: {e}");
                self.editor
                    .message(format!("The agent channel did not start: {e}"));
            }
        }
    }

    fn stop_agent_channel(&mut self) {
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
        self.agent_log("the agent channel is off");
        self.editor.message("Agent channel off");
    }

    /// Asks about the waiting connection once no prompt is open and no keys are in progress,
    /// and takes the person's answer once its question has closed.
    fn ask_about_a_connection(&mut self) {
        let Some(asking) = &self.agent.asking else {
            return;
        };
        match asking.question {
            None => {
                if self.editor.prompt().is_some() || self.dispatcher.reads_keys() {
                    return;
                }
                let label = connection_question(&asking.peer);
                let Ok(id) = self
                    .editor
                    .ask_with(label, "agent-channel-allow", CONNECTION_VERBS)
                else {
                    return;
                };
                self.editor.mark_from_agent(id);
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
                        .send(Err("the person did not let this agent in".into()));
                    self.agent_log(&format!("{who}: refused"));
                }
            }
        }
    }

    /// Whether `key` from the window must not reach a question the channel caused yet:
    /// it takes no answer until [`QUIET`] has passed with no key, and every key starts that
    /// wait again. `C-g` always goes through.
    pub(super) fn agent_question_holds(&mut self, key: KeyPress) -> bool {
        self.agent.person_typed = Some(Instant::now());
        if !self
            .editor
            .prompt()
            .is_some_and(|prompt| prompt.from_agent())
        {
            return false;
        }
        let quiet = self
            .agent
            .quiet_since
            .is_none_or(|at| at.elapsed() >= QUIET);
        self.agent.quiet_since = Some(Instant::now());
        if quiet || key == oso_core::keymap::CANCEL {
            return false;
        }
        self.editor.prompt_says("Agent: wait a moment, then answer");
        self.request_redraw();
        true
    }

    /// Text or a composition came from the window: the person is typing, and a question
    /// the channel caused waits longer.
    pub(super) fn person_typed(&mut self) {
        self.agent.person_typed = Some(Instant::now());
        if self
            .editor
            .prompt()
            .is_some_and(|prompt| prompt.from_agent())
        {
            self.agent.quiet_since = Some(Instant::now());
        }
    }

    /// Adds a row to `*Agent*`, with the time since the channel started.
    fn agent_log(&mut self, row: &str) {
        let since = self.agent.started.map_or(Duration::ZERO, |at| at.elapsed());
        let seconds = since.as_secs();
        self.editor
            .agent_log(&format!("+{:02}:{:02}  {row}", seconds / 60, seconds % 60));
    }
}

#[cfg(test)]
mod tests {
    use super::connection_question;
    use oso_platform::control::{Peer, PeerProcess};

    fn process(pid: u32, name: &str) -> Option<PeerProcess> {
        Some(PeerProcess::new(pid, Some(name)))
    }

    #[test]
    fn names_the_bridge_and_who_started_it_only_when_it_is_this_program() {
        let peer = Peer {
            process: process(815, "oso"),
            parent: process(812, "claude"),
            runs_this_program: true,
        };
        assert_eq!(
            connection_question(&peer),
            "let a process that reports itself as \"claude\" (pid 812) control oso through oso mcp (pid 815)?"
        );
        let impostor = Peer {
            runs_this_program: false,
            ..peer.clone()
        };
        assert_eq!(
            connection_question(&impostor),
            "let a process that reports itself as \"oso\" (pid 815) control oso?"
        );
    }

    #[test]
    fn an_adopted_or_unknown_parent_is_unknown() {
        for parent in [process(1, "launchd"), None] {
            let peer = Peer {
                process: process(815, "oso"),
                parent,
                runs_this_program: true,
            };
            assert!(connection_question(&peer).contains("an unknown process control oso"));
        }
        let unnamed = Peer {
            process: Some(PeerProcess::new(9, None)),
            parent: None,
            runs_this_program: false,
        };
        assert!(connection_question(&unnamed).contains("an unnamed process (pid 9)"));
        assert!(connection_question(&Peer::default()).contains("does not name"));
    }

    #[test]
    fn a_name_cannot_rewrite_the_question() {
        let peer = Peer {
            process: process(815, "oso\u{202e}?n ro y"),
            parent: None,
            runs_this_program: false,
        };
        let question = connection_question(&peer);
        assert!(question.is_ascii(), "{question}");
        assert!(question.contains("\\u{202e}"), "{question}");
    }
}
