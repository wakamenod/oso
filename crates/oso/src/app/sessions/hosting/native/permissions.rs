//! A native session's permission questions in the app (ADR 0165, step 2): the sessions' socket,
//! listened on while a native session runs or starts, on which only a running native session's
//! bridge is let in; and each `permission` it asks, put to the person once its call is found in
//! the session's own stream. The queue itself is `sessions::native::asks`.
//!
//! Must not depend on: the sockets' threads, which stay behind the channel's requests, or how a
//! frame is drawn. The socket is started on a thread of its own, as it binds in the file system.
//!
//! - What the person is asked about is the call as the stream has it, by its id and tool, and
//!   only if the request's input says the same of it: nothing a request says is drawn unless
//!   the stream says it too. The question names the session, the tool and its target, says
//!   when that is not all the call does, and `?` shows the input that runs whole, in a buffer
//!   withheld from agents. The question is the agent channel's kind, answered only from the
//!   window after a quiet moment, and its label is withheld from agents' state.
//! - Asked one at a time, the oldest first, and never while another agent question's answer is
//!   not taken yet; while one waits, its session is `needs input` with reason `permission`.
//! - The bridge going, the session ending, the socket stopping or the person's `C-g` denies:
//!   nothing runs unless the person said `y`. A second process that claims a session's slot,
//!   as a command the session runs can, is told no, and the person is told.

use std::sync::mpsc::Sender;
use std::time::Instant;

use oso_core::buffer::{Buffer, Mode};
use oso_platform::control::Peer;
use serde_json::Value;

use crate::app::{App, UserEvent};
use crate::channel::protocol::Op;
use crate::channel::protocol::permission::{self, Call, MAX_INPUT};
use crate::channel::{self, Connection, Event, Reply};
use crate::git::printable;
use crate::sessions::hosted::PermissionClaim;
use crate::sessions::identity::SessionId;
use crate::sessions::native::asks::{Ask, Asks, Denied, Look, Outcome, Question, Refused};
use crate::sessions::native::render;
use crate::sessions::native::rows::Tool;
use crate::sessions::native::wire::tools::{decode, only_keys, pretty};
use crate::sessions::state::{Reason, Target};
use crate::sessions::worker::SessionKey;

/// What the CLI is told when the person says no.
const DENIED: &str = "The person denied this call in oso.";

/// The buffer `?` shows a call's whole input in.
const DETAILS: &str = "*Session call*";

/// The most characters a question's label holds.
const MAX_LABEL: usize = 400;

/// The most bytes of a call's input `?` shows.
const MAX_SHOWN: usize = 4 * MAX_INPUT;

/// The sessions' socket and the questions asked on it.
#[derive(Debug, Default)]
pub(in crate::app) struct Permissions {
    socket: Option<channel::Channel>,
    starting: bool,
    /// Counted up each time the socket starts, so that nothing of a stopped run passes for a
    /// connection of the next.
    run: u64,
    asks: Asks<Sender<Reply>>,
    /// When the loop is woken next to deny a request whose call has not shown.
    wake_at: Option<Instant>,
    /// The buffer `?` shows the call in, made the first time and kept.
    details: Option<oso_core::buffer::BufferId>,
    /// What the window `?` showed it in showed before, while it shows it.
    before_details: Option<oso_core::buffer::BufferId>,
}

impl App {
    /// Starts the sessions' socket on a thread of its own, unless it runs or starts: a native
    /// session's bridge asks there, with the agent channel off or on.
    pub(super) fn start_sessions_socket(&mut self) {
        let permissions = &mut self.hosting.natives.permissions;
        if permissions.socket.is_some() || permissions.starting {
            return;
        }
        permissions.run += 1;
        let run = permissions.run;
        let waker = self.waker.clone();
        let spawned = std::thread::Builder::new()
            .name("oso-session-socket-start".into())
            .spawn(move || {
                let send = waker.clone();
                let started = channel::start_sessions(run, move |request| {
                    send.send(UserEvent::SessionSocket(request))
                });
                waker.send(UserEvent::SessionSocketStarted(started));
            });
        match spawned {
            Ok(_) => permissions.starting = true,
            Err(e) => log::warn!("native sessions: cannot start the sessions' socket: {e}"),
        }
    }

    /// The sessions' socket started, or could not: its sessions' calls that would ask are
    /// then denied, and the person told once. One no session needs any more stops at once.
    pub(in crate::app) fn sessions_socket_started(
        &mut self,
        started: std::io::Result<channel::Channel>,
    ) {
        self.hosting.natives.permissions.starting = false;
        match started {
            Ok(socket) => self.hosting.natives.permissions.socket = Some(socket),
            Err(e) => {
                log::warn!("native sessions: the sessions' socket did not start: {e}");
                self.editor.message(format!(
                    "Native sessions cannot ask you about their calls, which are denied: {e}"
                ));
            }
        }
        self.sync_sessions_socket();
    }

    /// After each event: the sessions' socket stops once no native session runs or starts,
    /// which denies what waits.
    pub(super) fn sync_sessions_socket(&mut self) {
        let natives = &self.hosting.natives;
        if natives.permissions.socket.is_none() {
            return;
        }
        let needed = (natives.map.iter()).any(|(key, native)| {
            native.stream.is_some() || self.hosting.starting.contains_key(key)
        });
        if needed {
            return;
        }
        // Dropping it closes every connection and removes the socket, on a thread.
        self.hosting.natives.permissions.socket = None;
        self.hosting.table.permission_bridges_gone();
        let gone = self.hosting.natives.permissions.asks.remove_where(|_| true);
        for ask in gone {
            self.drop_ask(ask);
        }
    }

    /// A connection of the sessions' socket brought something.
    pub(in crate::app) fn handle_session_socket(&mut self, request: channel::Request) {
        let channel::Request {
            connection,
            event,
            reply,
        } = request;
        let permissions = &self.hosting.natives.permissions;
        if permissions.socket.is_none() || connection.run != permissions.run {
            if let Some(reply) = reply {
                let _ = reply.send(Err("the sessions' socket stopped".into()));
            }
            return;
        }
        match (event, reply) {
            (
                Event::Claims {
                    peer,
                    session,
                    parent_started,
                },
                Some(reply),
            ) => self.permission_claim(connection, &peer, (&session, parent_started), reply),
            (Event::Op(Op::Permission(call)), Some(reply)) => {
                self.permission_asked(connection, call, reply);
            }
            (Event::Gone, _) => self.permission_bridge_gone(connection),
            (_, Some(reply)) => {
                let _ = reply.send(Err("the sessions' socket answers only permission".into()));
            }
            (_, None) => {}
        }
    }

    /// Lets `connection` in if it is a running native session's bridge, as a terminal
    /// session's bridge is let into the channel (ADR 0121). A second claimant of a session
    /// that holds one already is told no, and the person is told.
    fn permission_claim(
        &mut self,
        connection: Connection,
        peer: &Peer,
        (claim, parent_started): (&str, Option<std::time::SystemTime>),
        reply: Sender<Reply>,
    ) {
        let answer = |admitted: bool| reply.send(Ok(Value::Bool(admitted))).is_ok();
        let Some(id) = SessionId::parse(claim) else {
            answer(false);
            return;
        };
        self.refresh_running_children();
        let claimed = (self.hosting.table).answer_permission_claim(
            connection,
            peer,
            Some(&id),
            parent_started,
            answer,
        );
        let name = |key| (self.hosting.table.get(key)).map(|session| session.name.clone());
        match claimed {
            PermissionClaim::Holds(key) => {
                if let Some(name) = name(key) {
                    self.agent_log(&format!("session {name}'s bridge may ask permission"));
                }
            }
            PermissionClaim::Held(key) => {
                let Some(name) = name(key) else { return };
                let said = format!(
                    "A second process claimed session {name}'s permission bridge and was refused"
                );
                self.agent_log(&said);
                self.editor.message(said);
            }
            PermissionClaim::No => {}
        }
    }

    /// The bridge of `connection` asks about `call`: kept until it is found, asked and
    /// answered.
    fn permission_asked(&mut self, connection: Connection, call: Call, reply: Sender<Reply>) {
        let Some(key) =
            (self.hosting.table.by_permission_bridge(connection)).map(|session| session.key)
        else {
            let _ = reply.send(Err("this connection is no native session's bridge".into()));
            return;
        };
        let asks = &mut self.hosting.natives.permissions.asks;
        if let Some(refused) = asks.refuses(connection, key, &call.tool_use_id) {
            let why = match refused {
                Refused::Twice => "Denied: this session asks one call at a time.",
                Refused::Again => "Denied: oso was asked about this call already.",
            };
            let _ = reply.send(Ok(permission::deny(why)));
            return;
        }
        asks.push(Ask {
            from: connection,
            key,
            call,
            came: Instant::now(),
            question: None,
            asked: None,
            reply,
        });
        self.sync_permissions();
    }

    /// The connection went: what it asked is dropped, an open question closes, and its
    /// session holds no permission bridge.
    fn permission_bridge_gone(&mut self, connection: Connection) {
        self.hosting.table.permission_bridge_gone(connection);
        let gone =
            (self.hosting.natives.permissions.asks).remove_where(|ask| ask.from == connection);
        for ask in gone {
            self.drop_ask(ask);
        }
    }

    /// Native session `key` ended, or its buffer went: what it asked is dropped.
    pub(super) fn drop_permissions_of(&mut self, key: SessionKey) {
        let gone = (self.hosting.natives.permissions.asks).remove_where(|ask| ask.key == key);
        for ask in gone {
            self.drop_ask(ask);
        }
    }

    /// Denies `ask`, whose session, bridge or socket went: its question closes, and its
    /// session no longer waits on it.
    fn drop_ask(&mut self, ask: Ask<Sender<Reply>>) {
        if let Some(id) = ask.asked {
            self.orphan_agent_question(id);
            self.hide_permission_details();
        }
        if ask.question.is_some() {
            self.tell_tracker(ask.key, Target::Working);
        }
        let _ = (ask.reply).send(Ok(permission::deny(
            "Denied: oso could not ask the person.",
        )));
    }

    /// After each event, before the agent channel's questions, and after each batch of a
    /// native session's stream: takes the answer of the question that closed, finds the
    /// waiting requests' calls, denies those that cannot be asked, and asks the oldest found
    /// once the person is free.
    pub(in crate::app) fn sync_permissions(&mut self) {
        if self.hosting.natives.permissions.asks.is_empty() {
            return;
        }
        self.take_permission_answer();
        self.look_for_calls();
        self.ask_next_permission();
        self.wake_for_permissions();
    }

    /// The asked request whose question closed, answered as the person said.
    fn take_permission_answer(&mut self) {
        let prompts = self.editor.prompts();
        let open = |id| prompts.iter().any(|prompt| prompt.id() == id);
        let Some(ask) = self.hosting.natives.permissions.asks.take_closed(open) else {
            return;
        };
        self.hide_permission_details();
        let allowed = self.editor.take_agent_allowed();
        let said = if allowed { "allowed" } else { "denied" };
        self.agent_log(&format!(
            "a native session's {}: {said}",
            ask.call.tool_name
        ));
        self.tell_tracker(ask.key, Target::Working);
        let reply = if allowed {
            permission::allow()
        } else {
            permission::deny(DENIED)
        };
        let _ = ask.reply.send(Ok(reply));
    }

    /// Finds each request's call in its session's rows, and denies those that cannot be asked.
    fn look_for_calls(&mut self) {
        // Taken out while the rows are looked at, and put back.
        let mut asks = std::mem::take(&mut self.hosting.natives.permissions.asks);
        let (natives, table) = (&self.hosting.natives, &self.hosting.table);
        let outcomes = asks.look(Instant::now(), |ask| {
            let Some(native) = natives.map.get(&ask.key) else {
                return Look::Gone;
            };
            let Some(tool) = native.rows.waiting_call(&ask.call.tool_use_id) else {
                return Look::NotWaiting;
            };
            if tool.name != ask.call.tool_name {
                return Look::NotWaiting;
            }
            if ask.question.is_some() {
                return Look::Waits;
            }
            if !says_the_same(tool, ask.call.input.as_ref()) {
                return Look::Differs;
            }
            let Some(session) = table.get(ask.key) else {
                return Look::Gone;
            };
            let directory = native.rows.directory();
            Look::Found(question(
                &session.name,
                tool,
                directory,
                ask.call.input.as_ref(),
            ))
        });
        self.hosting.natives.permissions.asks = asks;
        for outcome in outcomes {
            match outcome {
                Outcome::Found(key) => {
                    self.tell_tracker(key, Target::NeedsInput(Reason::Permission))
                }
                Outcome::Denied(ask, why, found) => {
                    if found {
                        self.tell_tracker(ask.key, Target::Working);
                    }
                    let _ = ask.reply.send(Ok(permission::deny(denial(why))));
                }
            }
        }
    }

    /// Asks the oldest found request, once the person is free and no other agent question's
    /// answer waits to be taken: opening one forgets the last `y`.
    fn ask_next_permission(&mut self) {
        let free = self.agent_question_may_open() && self.session_tool_question().is_none();
        let Some(next) = self
            .hosting
            .natives
            .permissions
            .asks
            .next()
            .filter(|_| free)
        else {
            return;
        };
        let label = next
            .question
            .as_ref()
            .map(|q| q.label.clone())
            .unwrap_or_default();
        let came = next.came;
        match self.editor.ask_for_agent_withheld(
            label,
            "agent-channel-allow",
            crate::app::agent::AGENT_VERBS,
        ) {
            Ok(id) => {
                if let Some(next) = self.hosting.natives.permissions.asks.next() {
                    next.asked = Some(id);
                }
                log::debug!(
                    "native sessions: a permission question opened {} ms after its request",
                    came.elapsed().as_millis()
                );
                self.note_agent_question_opened();
                self.request_redraw();
            }
            Err(e) => {
                log::warn!("native sessions: cannot ask about a call: {e}");
                if let Some(ask) = self.hosting.natives.permissions.asks.take_next() {
                    self.drop_ask(ask);
                }
            }
        }
    }

    /// Wakes the loop when the first request whose call has not shown is to be denied, once:
    /// a request does not start a timer of its own.
    fn wake_for_permissions(&mut self) {
        let now = Instant::now();
        let permissions = &mut self.hosting.natives.permissions;
        if permissions.wake_at.is_some_and(|at| at <= now) {
            permissions.wake_at = None;
        }
        let Some(deadline) = permissions.asks.deadline() else {
            return;
        };
        if permissions.wake_at.is_some_and(|at| at <= deadline) {
            return;
        }
        permissions.wake_at = Some(deadline);
        self.wake_after(deadline.saturating_duration_since(now));
    }

    /// The open permission question, if one is: a question of its own, which no connection's
    /// follow-up takes.
    pub(in crate::app) fn permission_question(&self) -> Option<oso_core::minibuffer::PromptId> {
        self.hosting
            .natives
            .permissions
            .asks
            .open()
            .and_then(|ask| ask.asked)
    }

    /// `?` in an open permission question: `*Session call*` shows the call's whole input,
    /// withheld from agents as the session's buffer is. Whether one was open.
    pub(in crate::app) fn show_permission_details(&mut self) -> bool {
        let Some(details) = (self.hosting.natives.permissions.asks.open())
            .and_then(|ask| ask.question.as_ref())
            .map(|question| question.details.clone())
        else {
            return false;
        };
        let kept = self.hosting.natives.permissions.details;
        let id = match kept.filter(|&id| self.editor.buffer(id).is_some_and(Buffer::is_internal)) {
            Some(id) => id,
            None => self.editor.add_internal_buffer(DETAILS, Mode::Fundamental),
        };
        self.hosting.natives.permissions.details = Some(id);
        if let Some(buffer) = self.editor.buffer_mut(id) {
            buffer.withhold_from_agents();
        }
        self.editor.set_internal_text(id, &details);
        let before: Vec<_> = (self.editor.windows())
            .map(|w| (w.id(), w.buffer()))
            .collect();
        self.editor.show(id);
        // The window it went to shows what it showed again once the question closes.
        let shown_in = (self.editor.windows())
            .find(|window| window.buffer() == id)
            .and_then(|window| before.iter().find(|(was, _)| *was == window.id()))
            .map(|&(_, buffer)| buffer)
            .filter(|&buffer| buffer != id);
        if shown_in.is_some() {
            self.hosting.natives.permissions.before_details = shown_in;
        }
        self.request_redraw();
        true
    }

    /// The question closed: the window `?` showed the call in shows what it showed before,
    /// and the call's text goes. The buffer is kept for the next `?`.
    fn hide_permission_details(&mut self) {
        let permissions = &mut self.hosting.natives.permissions;
        let (Some(id), before) = (permissions.details, permissions.before_details.take()) else {
            return;
        };
        self.editor.set_internal_text(id, "");
        if self.editor.current().0.buffer() == id
            && let Some(before) = before.filter(|&before| self.editor.buffer(before).is_some())
        {
            self.editor.show(before);
        }
    }
}

/// Whether the request's `input` says of the call what the stream's `tool` does: the same
/// target, as both are read the same way. A request with no input, as its input was too large
/// to send, says the same only of a call whose input the stream found too large too.
fn says_the_same(tool: &Tool, input: Option<&Value>) -> bool {
    let Some(input) = input else {
        return tool.input.cut;
    };
    let sent = decode(&tool.name, Some(input));
    let kept = &tool.input;
    (sent.path == kept.path && sent.text == kept.text)
        && (sent.kind == kept.kind && sent.items == kept.items)
}

/// The question about the call `tool` of session `name`, which runs in `directory`, with the
/// request's `input`: its label, and what `?` shows.
fn question(
    name: &str,
    tool: &Tool,
    directory: &std::path::Path,
    input: Option<&Value>,
) -> Question {
    let target = render::target(tool, directory);
    let all = !target.ends_with('…') && input.is_some_and(|input| shows_all(&tool.name, input));
    let run = if target.is_empty() {
        format!("Session {name} asks to run {}", tool.name)
    } else {
        format!("Session {name} asks to run {} {target}", tool.name)
    };
    let more = if all {
        ""
    } else {
        " (not all of it is shown: ? shows the call)"
    };
    let label = printable(&format!("{run}: allow it?{more}"), MAX_LABEL);
    let body = input.map_or_else(
        || {
            format!(
                "Its input is over {} KiB as JSON and is not shown here.",
                MAX_INPUT >> 10
            )
        },
        |input| pretty(input, MAX_SHOWN),
    );
    let shown: Vec<String> = body
        .lines()
        .map(|line| printable(line, MAX_SHOWN))
        .collect();
    let details = format!(
        "Session {name} asks to run {}:\n\n{}\n\ny allows it, n or C-g denies it.\n",
        tool.name,
        shown.join("\n")
    );
    Question { label, details }
}

/// Whether what a question shows of a call of `name`, its target, is all its `input` does:
/// only for the calls that read, and a command with nothing but its description and limit.
fn shows_all(name: &str, input: &Value) -> bool {
    match name {
        "Read" | "Glob" | "Grep" | "WebSearch" | "WebFetch" => true,
        "Bash" => only_keys(input, &["command", "description", "timeout"]),
        _ => false,
    }
}

/// What the CLI is told of a request denied unasked.
fn denial(why: Denied) -> &'static str {
    match why {
        Denied::NotFound => "Denied: oso found no such call waiting in the session's stream.",
        Denied::NoLonger => "Denied: the call was no longer waiting when oso could ask.",
        Denied::Differs => {
            "Denied: the call oso was asked about is not the one the session's stream shows."
        }
        Denied::Gone => "Denied: oso could not ask the person.",
    }
}

#[cfg(test)]
mod tests;
