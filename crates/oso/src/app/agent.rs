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

use oso_core::command::Reach;
use oso_core::editor::AgentChannel;
use oso_core::key::KeyPress;
use oso_core::minibuffer::PromptId;
use oso_platform::control::Peer;

mod admit;
mod open;

use super::App;
use super::shell::Loop;
use crate::channel::protocol::{Arg, Op};
use crate::channel::{self, Connection, Event, Reply};
use crate::dispatch::Given;

/// How long a question the channel caused takes no answer after the last key.
pub const QUIET: Duration = Duration::from_millis(500);

/// How often a row that can come many times a second is logged, at most.
const OFTEN: Duration = Duration::from_secs(5);

/// The most bytes a text argument may come to, times the cursors it goes in at (ADR 0085).
pub const MAX_INSERTED: usize = 1 << 20;

/// How long after the person's last key the channel runs nothing.
pub const PERSON_TYPING: Duration = Duration::from_secs(1);

/// The verbs of the connection's question, after `y` and `n`.
const CONNECTION_VERBS: [&str; 2] = ["allow", "refuse"];

/// The agent channel's part of the app.
#[derive(Debug, Default)]
pub(super) struct Agent {
    channel: Option<channel::Channel>,
    /// The channel is starting on a thread of its own.
    starting: bool,
    /// The person turned it off while it was starting.
    stop_when_started: bool,
    started: Option<Instant>,
    /// The connections the person let in.
    connected: Vec<Connection>,
    /// The connection that waits for the person, and its question once it is asked.
    asking: Option<Asking>,
    /// The questions the channel's commands caused, with the connection that caused each.
    caused: Vec<(PromptId, Connection)>,
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
    keys_in_progress: bool,
    composing: bool,
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
    if now.keys_in_progress {
        return Some("the person is in the middle of a key sequence");
    }
    if now.composing {
        return Some("the person is composing text");
    }
    if now.since_typed.is_some_and(|since| since < PERSON_TYPING) {
        return Some("the person is typing; try again in a moment");
    }
    None
}

/// Whether `key` must not reach a question the agent caused, `since` the last key or its
/// opening: it takes none until [`QUIET`] has passed. `C-g` always goes through.
fn holds(since: Option<Duration>, key: KeyPress) -> bool {
    key != oso_core::keymap::CANCEL && since.is_some_and(|since| since < QUIET)
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
                    op => {
                        let _ = reply.send(self.agent_op(event_loop, connection, op));
                    }
                }
            }
        }
    }

    /// A connection closed, or the channel stopped under it.
    fn agent_gone(&mut self, connection: Connection) {
        self.agent.connected.retain(|&open| open != connection);
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

    /// Answers a request of a connection the person let in.
    fn agent_op(&mut self, event_loop: &Loop<'_>, connection: Connection, op: Op) -> Reply {
        match op {
            Op::State { buffer_offset } => {
                self.agent_log_often("state");
                Ok(channel::state::state(&self.editor, buffer_offset))
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
            Op::Run { name, arg, reach } => {
                self.agent_run(event_loop, connection, &name, arg, reach)
            }
            // Answered where they come in, as they reply later.
            Op::Text(_) | Op::Open { .. } => Err("not here".into()),
        }
    }

    /// Runs a command for the channel, unless the person is in the middle of something.
    fn agent_run(
        &mut self,
        event_loop: &Loop<'_>,
        connection: Connection,
        name: &str,
        arg: Option<Arg>,
        reach: Reach,
    ) -> Reply {
        if let Some(why) = self.agent_waits() {
            self.agent_log_often(why);
            return Err(why.into());
        }
        let given = match &arg {
            None => Given::Nothing,
            Some(Arg::Count(n)) => Given::Count(*n),
            Some(Arg::Text(text)) => Given::Text(text),
        };
        // Text goes in at every cursor, so its size times theirs is what it costs.
        if let Given::Text(text) = given
            && text
                .len()
                .saturating_mul(self.editor.window().selections.len())
                > MAX_INSERTED
        {
            return Err(format!(
                "the text times the cursors is more than {MAX_INSERTED} bytes"
            ));
        }
        let newest = self.editor.prompts().last().map(|prompt| prompt.id());
        let ran = self
            .dispatcher
            .run_for_agent(&mut self.editor, name, given, reach);
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
        let asked = self.note_caused(newest, connection);
        self.apply(event_loop, effects);
        // A text argument is never logged, only its length.
        let shown_arg = match &arg {
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
    pub(super) fn agent_waits(&self) -> Option<&'static str> {
        let prompts = self.editor.prompts();
        waits(&Now {
            shows_secret: self.editor.shows_secret(),
            agents_question: prompts.iter().any(|prompt| prompt.from_agent()),
            persons_prompt: !prompts.is_empty(),
            keys_in_progress: self.dispatcher.keys_in_progress(),
            composing: self.composing(),
            since_typed: self.agent.person_typed.map(|at| at.elapsed()),
        })
    }

    /// Notes the prompts a channel command opened, those newer than `newest`, which are the
    /// agent's, and returns the innermost one's label if it opened any.
    fn note_caused(&mut self, newest: Option<PromptId>, connection: Connection) -> Option<String> {
        let opened: Vec<PromptId> = self
            .editor
            .prompts()
            .iter()
            .filter(|prompt| prompt.from_agent() && newest.is_none_or(|n| prompt.id() > n))
            .map(|prompt| prompt.id())
            .collect();
        for &id in &opened {
            self.agent.caused.push((id, connection));
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
        self.close_orphaned_questions();
        self.progress_agent_opens();
        let open = self.editor.prompts();
        self.agent
            .caused
            .retain(|(id, _)| open.iter().any(|prompt| prompt.id() == *id));
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

    /// Whether `key` from the window must not reach a question the channel caused yet:
    /// it takes no answer until [`QUIET`] has passed with no key, and every key starts that
    /// wait again. `C-g` always goes through.
    pub(super) fn note_key_for_agent(&mut self, key: KeyPress) -> bool {
        self.agent.person_typed = Some(Instant::now());
        if !self
            .editor
            .prompt()
            .is_some_and(|prompt| prompt.from_agent())
        {
            return false;
        }
        let since = self.agent.quiet_since.map(|at| at.elapsed());
        self.agent.quiet_since = Some(Instant::now());
        if !holds(since, key) {
            return false;
        }
        self.editor.prompt_says("Agent: wait a moment, then answer");
        self.request_redraw();
        true
    }

    /// Why keys from the dev socket, which stand in for the person's, must not reach a
    /// question the agent caused, if they must not: they come one at a time, so that each one
    /// restarts its wait, and each one waits as a key from the window does.
    #[cfg(feature = "dev")]
    pub(super) fn agent_holds_keys(&mut self, keys: &[KeyPress]) -> Option<&'static str> {
        let agents = self
            .editor
            .prompt()
            .is_some_and(|prompt| prompt.from_agent());
        if agents && keys.len() > 1 {
            return Some("send keys one at a time to a question the agent caused");
        }
        let held = keys
            .first()
            .is_some_and(|&key| self.note_key_for_agent(key));
        held.then_some("a question the agent caused takes no answer yet")
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

    /// The questions `connection` caused, or every connection if `None`, take no answer any
    /// more: each closes once it is innermost.
    fn orphan_questions(&mut self, connection: Option<Connection>) {
        let (gone, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.agent.caused)
            .into_iter()
            .partition(|(_, by)| connection.is_none_or(|c| c == *by));
        self.agent.caused = kept;
        self.agent
            .orphaned
            .extend(gone.into_iter().map(|(id, _)| id));
        self.close_orphaned_questions();
    }

    /// Closes the orphaned questions that are innermost, as `C-g` would.
    fn close_orphaned_questions(&mut self) {
        while let Some(id) = self.editor.prompt().map(|prompt| prompt.id())
            && self.agent.orphaned.contains(&id)
        {
            self.editor.quit();
            self.request_redraw();
        }
        let open = self.editor.prompts();
        self.agent
            .orphaned
            .retain(|id| open.iter().any(|prompt| prompt.id() == *id));
    }

    /// Logs `row`, which can come many times a second, at most once every few seconds, with
    /// how many came since.
    fn agent_log_often(&mut self, row: &'static str) {
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
    use super::{Now, PERSON_TYPING, QUIET, holds, waits};
    use oso_core::key::{Key, KeyPress};
    use oso_core::keymap::CANCEL;
    use std::time::Duration;

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

    #[test]
    fn a_question_takes_no_key_until_a_quiet_moment_but_c_g() {
        let y = KeyPress::plain(Key::Char('y'));
        assert!(holds(Some(Duration::from_millis(100)), y));
        assert!(holds(Some(QUIET - Duration::from_millis(1)), y));
        assert!(!holds(Some(QUIET), y));
        assert!(!holds(None, y));
        assert!(!holds(Some(Duration::ZERO), CANCEL));
    }
}
