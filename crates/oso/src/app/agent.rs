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

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde_json::json;

use oso_core::buffer::BufferId;
use oso_core::command::Reach;
use oso_core::editor::AgentChannel;
use oso_core::minibuffer::PromptId;
use oso_platform::control::{Peer, PeerProcess};

mod admit;
mod open;
pub(super) use open::TOO_LARGE_FOR_THE_AGENT;
mod questions;
mod saving;
mod undoing;

use super::App;
use super::shell::Loop;
use crate::channel::protocol::{Arg, Op};
use crate::channel::{self, Connection, Event, Reply};
use crate::dispatch::Given;

/// How long a question the channel caused takes no answer after the last key.
pub const QUIET: Duration = Duration::from_millis(500);

/// How often a row that can come many times a second is logged, at most.
const OFTEN: Duration = Duration::from_secs(5);

/// The most bytes of text one connection gives its commands in a minute.
pub const MAX_TEXT_A_MINUTE: usize = 4 << 20;

/// How long after the person's last key the channel runs nothing.
pub const PERSON_TYPING: Duration = Duration::from_secs(1);

/// The verbs of the connection's question, after `y` and `n`, and of a session tool's.
pub(in crate::app) const AGENT_VERBS: [&str; 2] = ["allow", "refuse"];

/// The agent channel's part of the app.
#[derive(Debug, Default)]
pub(super) struct Agent {
    channel: Option<channel::Channel>,
    /// The channel is starting on a thread of its own.
    starting: bool,
    /// The person turned it off while it was starting.
    stop_when_started: bool,
    /// The configuration file, by this name, started it at launch (ADR 0108).
    started_by: Option<String>,
    /// `claude` started it, for its session's bridge (ADR 0121, Q20).
    started_for_a_session: bool,
    /// It is on because `claude` turned it on, so a start that is refused turns it off again
    /// while no session needs it.
    on_for_sessions: bool,
    started: Option<Instant>,
    /// The connections the person let in.
    connected: Vec<Connection>,
    /// The connection that waits for the person, and its question once it is asked.
    asking: Option<Asking>,
    /// The questions the channel's commands caused, with the connection that caused each.
    caused: Vec<(PromptId, Connection)>,
    /// The connection whose question the person answered last, which a question that answer
    /// opens is put down to.
    last_answered: Option<Connection>,
    /// Questions the agent caused whose connection went, or whose channel stopped, which
    /// close as soon as they are innermost: no `y` answers them any more.
    orphaned: Vec<PromptId>,
    /// What was last logged of a kind that can come many times a second, and how many have
    /// come since: reads and waits.
    often: HashMap<&'static str, (Option<Instant>, usize)>,
    /// The channel's run: one more each time it starts, so that nothing of a stopped run
    /// passes for the current one.
    run: u64,
    /// When a key last came while a question the channel caused was innermost, or when it
    /// opened.
    quiet_since: Option<Instant>,
    /// When the person last typed in the window.
    person_typed: Option<Instant>,
    /// The files the channel is opening.
    opening: Vec<open::Opening>,
    /// How many files each connection has opened.
    opened: HashMap<Connection, usize>,
    /// The text each connection has given this minute: since when, and how many bytes.
    inserted: HashMap<Connection, (Instant, usize)>,
    /// Requests whose reply waits for the save their command started (ADR 0108).
    saving: Vec<saving::Saving>,
    /// The request whose reply waits for the undo or the redo its command began (ADR 0119).
    undoing: Option<undoing::Waiting>,
}

#[derive(Debug)]
struct Asking {
    connection: Connection,
    peer: Peer,
    reply: std::sync::mpsc::Sender<Reply>,
    question: Option<PromptId>,
}

/// What decides whether the channel may run something now.
#[derive(Debug, Clone, Copy, Default)]
struct Now {
    shows_secret: bool,
    agents_question: bool,
    persons_prompt: bool,
    /// The person has a menu open (ADR 0117).
    in_menu: bool,
    keys_in_progress: bool,
    composing: bool,
    /// An isearch, a jump or a query-replace is on (ADR 0082, ADR 0113, ADR 0116).
    searching: bool,
    /// A keyboard macro is being recorded or replayed (ADR 0113).
    macro_running: bool,
    /// An undo or a redo of a large step goes on (ADR 0119).
    undoing: bool,
    /// How long since the person last typed, if they have.
    since_typed: Option<Duration>,
}

/// Why the channel may run nothing `now`, if it may not (ADR 0085).
fn waits(now: &Now) -> Option<&'static str> {
    if now.shows_secret {
        return Some("a buffer that may hold secrets is shown; ask the person");
    }
    if now.agents_question {
        return Some("a question the agent caused waits for the person");
    }
    if now.persons_prompt {
        return Some("the person is answering a prompt");
    }
    if now.in_menu {
        return Some("the person is in a menu");
    }
    if now.keys_in_progress {
        return Some("the person is in the middle of a key sequence");
    }
    if now.composing {
        return Some("the person is composing text");
    }
    if now.searching {
        return Some("the person is searching, jumping or replacing");
    }
    if now.macro_running {
        return Some("the person is recording or running a keyboard macro");
    }
    if now.undoing {
        return Some("an undo or a redo goes on; try again in a moment");
    }
    if now.since_typed.is_some_and(|since| since < PERSON_TYPING) {
        return Some("the person is typing; try again in a moment");
    }
    None
}

impl App {
    /// Turns the agent channel on for a hosted session's bridge, if it is off, and says so
    /// (ADR 0121, Q20): the person asked for a session whose `oso mcp` needs it.
    pub(super) fn turn_agent_channel_on_for_a_session(&mut self) {
        if self.agent.channel.is_some() || self.agent.starting {
            return;
        }
        self.start_agent_channel();
        self.agent.started_for_a_session = self.agent.starting;
    }

    /// Turns the agent channel off again after a session's start was refused, if a session's
    /// start turned it on and nothing holds it now: no session runs or starts, and no
    /// connection is let in.
    pub(super) fn turn_agent_channel_off_unless_a_session_needs_it(&mut self, needed: bool) {
        let for_sessions =
            self.agent.on_for_sessions || (self.agent.starting && self.agent.started_for_a_session);
        if for_sessions && !needed && self.agent.connected.is_empty() {
            self.stop_agent_channel();
        }
    }

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
        if self.agent.channel.is_none() || connection.run != self.agent.run {
            if let Some(reply) = reply {
                let _ = reply.send(Err("the agent channel stopped".into()));
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
            // A hosted session's bridge is let in without the question, as that session's
            // (ADR 0121, Q19), and logged as any connection.
            Event::Claims { peer, session } => {
                if let Some(reply) = reply {
                    self.agent_claims(connection, &peer, &session, reply);
                }
            }
            Event::Gone => self.agent_gone(connection),
            Event::Silent => self.agent_log_often("closed connections that sent nothing"),
            Event::Op(op) => {
                let Some(reply) = reply else { return };
                if !self.agent.connected.contains(&connection) {
                    let _ = reply.send(Err("the person has not let this connection in".into()));
                    return;
                }
                match op {
                    // These reply later: once the text is copied, or the file shown.
                    Op::Text(of) => {
                        self.agent_log_often("text");
                        match channel::text::start(&self.editor, of) {
                            Ok(job) => job.spawn(reply),
                            Err(why) => {
                                let _ = reply.send(Err(why));
                            }
                        }
                    }
                    Op::Open { path, line } => self.agent_open(connection, path, line, reply),
                    // Each replies when it can: at once, after a job, or after the person's
                    // answer and what it started (ADR 0121, step 6).
                    Op::Session(call) => self.session_tool(connection, call, reply),
                    // This replies once the save its command started has ended, if it started
                    // one, or the undo it began (ADR 0119).
                    Op::Run { name, arg, reach } => {
                        self.agent_run_and_reply(
                            event_loop,
                            connection,
                            (&name, arg, reach),
                            reply,
                        );
                    }
                    op => {
                        let _ = reply.send(self.agent_op(connection, op));
                    }
                }
            }
        }
    }

    /// Runs a command for the channel and replies: at once, or once the save its command
    /// started has ended, or the undo it began (ADR 0108, ADR 0119).
    fn agent_run_and_reply(
        &mut self,
        event_loop: &Loop<'_>,
        connection: Connection,
        (name, arg, reach): (&str, Option<Arg>, Reach),
        reply: std::sync::mpsc::Sender<Reply>,
    ) {
        let undoing = self.editor.crossing().is_some();
        // The steps an undo asks to cross, as the command bounds them.
        let steps = match &arg {
            Some(Arg::Count(n)) => n.unsigned_abs().clamp(1, crate::commands::MAX_CROSSINGS),
            _ => 1,
        };
        let (ran, saves) = self.agent_run(event_loop, connection, name, arg, reach);
        // An undo that goes on replies once it has ended. One under way before was not this
        // command's: the channel waited for it.
        let Some((reply, ran)) = (if undoing {
            Some((reply, ran))
        } else {
            self.wait_for_undo(connection, reply, (ran, steps))
        }) else {
            return;
        };
        match (ran, saves.as_slice()) {
            (Ok(done), [buffer, ..]) => self.wait_for_save(connection, *buffer, reply, done),
            (ran, _) => {
                let _ = reply.send(ran);
            }
        }
    }

    /// Wakes the loop once `after` has passed, as nothing else may.
    pub(super) fn wake_after(&self, after: Duration) {
        let waker = self.waker.clone();
        let spawned = std::thread::Builder::new()
            .name("oso-agent-deadline".into())
            .spawn(move || {
                std::thread::sleep(after);
                waker.send(super::UserEvent::AgentDeadline);
            });
        if let Err(e) = spawned {
            log::warn!("agent channel: cannot start the deadline thread: {e}");
        }
    }

    /// A connection closed, or the channel stopped under it.
    fn agent_gone(&mut self, connection: Connection) {
        self.agent.connected.retain(|&open| open != connection);
        self.session_tools_gone(Some(connection));
        self.session_bridge_gone(Some(connection));
        saving::forget_saves_of(&mut self.agent.saving, connection);
        self.forget_undo_of(Some(connection));
        self.orphan_questions(Some(connection));
        self.drop_agent_opens(Some(connection));
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

    /// The connection of `peer` claims to be the bridge of the hosted session `session`: it is
    /// let in unasked if it is, and the server is told.
    fn agent_claims(
        &mut self,
        connection: Connection,
        peer: &Peer,
        session: &str,
        reply: std::sync::mpsc::Sender<Reply>,
    ) {
        // An answer that comes after the server stopped waiting is undone: the server treats
        // the connection as any agent's, which asks the person, and it holds no session.
        let answer = |bridges: bool| reply.send(Ok(serde_json::Value::Bool(bridges))).is_ok();
        if let Some(name) = self.session_bridge(connection, peer, session, answer) {
            self.agent.connected.push(connection);
            let pid = peer.process.as_ref().map_or(0, PeerProcess::pid);
            self.agent_log(&format!("pid {pid}: let in as session {name}'s bridge"));
            self.request_redraw();
        }
    }

    /// Answers a request of a connection the person let in.
    fn agent_op(&mut self, connection: Connection, op: Op) -> Reply {
        match op {
            Op::State { buffer_offset } => {
                self.agent_log_often("state");
                Ok(channel::state::state(
                    &self.editor,
                    self.dispatcher.menu_name(),
                    buffer_offset,
                ))
            }
            Op::Commands => {
                self.agent_log_often("commands");
                Ok(channel::state::commands(self.dispatcher.commands()))
            }
            Op::Cancel => {
                let innermost = self.editor.prompt().map(|prompt| prompt.id());
                let own = |id: &PromptId| self.agent.caused.contains(&(*id, connection));
                match innermost.filter(own) {
                    Some(_) => {
                        self.editor.quit();
                        self.request_redraw();
                        self.agent_log("cancel: ok");
                        Ok(json!({ "cancelled": true }))
                    }
                    None => Err("no question this agent caused is open".into()),
                }
            }
            // Answered where they come in, as they reply later.
            Op::Text(_) | Op::Open { .. } | Op::Run { .. } | Op::Session(_) => {
                Err("not here".into())
            }
        }
    }

    /// Runs a command for the channel, unless the person is in the middle of something.
    /// Returns the reply, and the buffers whose saves it started, which the reply waits for.
    fn agent_run(
        &mut self,
        event_loop: &Loop<'_>,
        connection: Connection,
        name: &str,
        arg: Option<Arg>,
        reach: Reach,
    ) -> (Reply, Vec<BufferId>) {
        // What the channel may never run is said at once, before anything waits.
        if let Err(refused) = self.dispatcher.by_reach(name, reach) {
            self.agent_log(&format!("run: refused: {}", refused.0));
            return (Err(refused.0), Vec::new());
        }
        if let Some(why) = self.agent_waits() {
            self.agent_log_often(why);
            return (Err(why.into()), Vec::new());
        }
        self.agent_run_now(event_loop, connection, name, arg.as_ref(), reach)
    }

    /// [`Self::agent_run`], once nothing makes the channel wait.
    fn agent_run_now(
        &mut self,
        event_loop: &Loop<'_>,
        connection: Connection,
        name: &str,
        arg: Option<&Arg>,
        reach: Reach,
    ) -> (Reply, Vec<BufferId>) {
        let given = match arg {
            None => Given::Nothing,
            Some(Arg::Count(n)) => Given::Count(*n),
            Some(Arg::Text(text)) => Given::Text(text),
        };
        // What one connection puts in a minute is bounded, as well as what one request does.
        if let Given::Text(text) = given {
            let now = Instant::now();
            let (since, bytes) = self.agent.inserted.entry(connection).or_insert((now, 0));
            if now.duration_since(*since) >= Duration::from_secs(60) {
                (*since, *bytes) = (now, 0);
            }
            if bytes.saturating_add(text.len()) > MAX_TEXT_A_MINUTE {
                let full = "this agent has put in as much text as it may this minute";
                return (Err(full.into()), Vec::new());
            }
            *bytes += text.len();
        }
        let newest = self.editor.prompts().last().map(|prompt| prompt.id());
        let ran = self
            .dispatcher
            .run_for_agent(&mut self.editor, name, given, reach);
        let (effects, result) = match ran {
            Ok(ran) => ran,
            Err(refused) => {
                self.agent_log(&format!("run: refused: {}", refused.0));
                return (Err(refused.0), Vec::new());
            }
        };
        // The saves it queued are the agent's, and its reply waits for them (ADR 0108).
        let (saves, not_started) = self.start_agent_saves();
        // What the agent killed stays in oso's ring: never the OS clipboard (ADR 0085).
        drop(self.editor.take_clipboard_write());
        self.note_logging();
        let asked = self.note_caused(newest, connection);
        self.apply(event_loop, effects);
        // A text argument is never logged, only its length.
        let shown_arg = match arg {
            None => String::new(),
            Some(Arg::Count(n)) => format!(" {n}"),
            Some(Arg::Text(text)) => format!(" ({} bytes of text)", text.len()),
        };
        let outcome = match (&result, &asked) {
            (Err(message), _) => format!("failed: {message}"),
            (Ok(()), Some(_)) => "waiting for the person".to_owned(),
            (Ok(()), None) => "ok".to_owned(),
        };
        self.agent_log(&format!("run {name}{shown_arg}: {outcome}"));
        let reply = match (result, asked) {
            (Err(message), _) => Err(message),
            (Ok(()), Some(label)) => Ok(json!({
                "command": name,
                "waiting": format!("waiting for the person: {label}"),
            })),
            (Ok(()), None) => Ok(json!({ "command": name })),
        };
        match not_started {
            Some(why) => (Err(why.into()), Vec::new()),
            None => (reply, saves),
        }
    }

    /// Why the channel may run nothing now, if it may not.
    pub(super) fn agent_waits(&self) -> Option<&'static str> {
        let prompts = self.editor.prompts();
        waits(&Now {
            shows_secret: self.editor.shows_secret(),
            agents_question: prompts.iter().any(|prompt| prompt.from_agent()),
            persons_prompt: !prompts.is_empty(),
            in_menu: self.dispatcher.menu_open(),
            keys_in_progress: self.dispatcher.keys_in_progress(),
            composing: self.composing(),
            searching: self.editor.isearch().is_some()
                || self.editor.jump().is_some()
                || self.editor.replace().is_some()
                || self.editor.indent_rigidly_layer(),
            macro_running: self.editor.macro_recording() || self.editor.macro_replaying(),
            undoing: self.editor.crossing().is_some(),
            since_typed: self.agent.person_typed.map(|at| at.elapsed()),
        })
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
        // A session tool's answer is taken first: asking about a connection forgets the last
        // `y`, which may be the person's to the tool's question.
        self.sync_session_tools();
        self.ask_about_a_connection();
        self.close_orphaned_questions();
        self.progress_agent_opens();
        self.reply_to_long_saves();
        self.track_follow_up_questions();
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

    /// Logs `row`, which can come many times a second, at most once every few seconds, with
    /// how many came since.
    pub(in crate::app) fn agent_log_often(&mut self, row: &'static str) {
        let now = Instant::now();
        let (at, count) = self.agent.often.entry(row).or_insert((None, 0));
        *count += 1;
        if at.is_none_or(|at| now.duration_since(at) >= OFTEN) {
            let count = std::mem::take(count);
            *at = Some(now);
            let times = if count > 1 {
                format!(" ({count} times)")
            } else {
                String::new()
            };
            self.agent_log(&format!("{row}{times}"));
        }
    }

    /// Adds a row to `*Agent*`, with the time since the channel started.
    pub(super) fn agent_log(&mut self, row: &str) {
        let since = self.agent.started.map_or(Duration::ZERO, |at| at.elapsed());
        let seconds = since.as_secs();
        self.editor
            .agent_log(&format!("+{:02}:{:02}  {row}", seconds / 60, seconds % 60));
    }
}

#[cfg(test)]
mod tests {
    use super::{Now, PERSON_TYPING, waits};
    use std::time::Duration;

    #[test]
    fn the_channel_waits_while_the_person_has_a_menu_open() {
        let now = Now {
            in_menu: true,
            ..Now::default()
        };
        assert_eq!(waits(&now), Some("the person is in a menu"));
    }

    #[test]
    fn the_channel_waits_for_each_thing_the_person_is_in_the_middle_of() {
        assert_eq!(waits(&Now::default()), None);
        let typed = Some(Duration::from_millis(300));
        let cases = [
            (
                Now {
                    shows_secret: true,
                    ..Now::default()
                },
                "may hold secrets",
            ),
            (
                Now {
                    agents_question: true,
                    ..Now::default()
                },
                "waits for the person",
            ),
            (
                Now {
                    persons_prompt: true,
                    ..Now::default()
                },
                "answering a prompt",
            ),
            (
                Now {
                    keys_in_progress: true,
                    ..Now::default()
                },
                "key sequence",
            ),
            (
                Now {
                    composing: true,
                    ..Now::default()
                },
                "composing",
            ),
            (
                Now {
                    searching: true,
                    ..Now::default()
                },
                "searching",
            ),
            (
                Now {
                    macro_running: true,
                    ..Now::default()
                },
                "keyboard macro",
            ),
            (
                Now {
                    undoing: true,
                    ..Now::default()
                },
                "undo or a redo",
            ),
            (
                Now {
                    since_typed: typed,
                    ..Now::default()
                },
                "typing",
            ),
        ];
        for (now, why) in cases {
            assert!(
                waits(&now).is_some_and(|said| said.contains(why)),
                "{now:?}"
            );
        }
        let long_ago = Now {
            since_typed: Some(PERSON_TYPING),
            ..Now::default()
        };
        assert_eq!(waits(&long_ago), None);
    }
}
