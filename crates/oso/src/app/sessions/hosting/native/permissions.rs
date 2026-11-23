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
//!   the stream says it too, but what only the request carries: an `ExitPlanMode`'s plan, and
//!   the fields the stream's reading of a call leaves out (a description, an edit's strings, a
//!   question's text). It is asked on the session's card (step 2e-2, `card`), withheld from
//!   agents, answered only from the window after a quiet moment; `RET` shows in the card the
//!   input that runs, whole.
//! - Each session's call has its card, as each session holds one connection; while one waits,
//!   its session is `needs input` with reason `permission`, `question` or `plan`.
//! - The bridge going, the session ending or the socket stopping denies: nothing runs unless
//!   the person said `y`, or answered the questions. A second process that claims a session's slot,
//!   as a command the session runs can, is told no, and the person is told.

use std::sync::mpsc::Sender;
use std::time::Instant;

mod card;

pub(super) use card::Writing;
use oso_platform::control::Peer;
use serde_json::Value;

use crate::app::{App, UserEvent};
use crate::channel::protocol::Op;
use crate::channel::protocol::permission::{self, Call};
use crate::channel::{self, Connection, Event, Reply};
use crate::sessions::hosted::PermissionClaim;
use crate::sessions::identity::SessionId;
use crate::sessions::native::asks::{Ask, Asks, Denied, Look, Outcome, Refused};
use crate::sessions::native::card::question;
use crate::sessions::native::rows::Tool;
use crate::sessions::native::wire::tools::decode;
use crate::sessions::state::{Reason, Target};
use crate::sessions::worker::SessionKey;

/// What the CLI is told when the person says no.
const DENIED: &str = "The person denied this call in oso.";

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
    /// The sessions whose card point has not gone to since it was shown, and when that was
    /// (step 2e-2, ADR 0180).
    focus_pending: Vec<(SessionKey, Instant)>,
    /// When the loop last looked for a card to move point to.
    looked: Option<Instant>,
    /// Each shown card's width as drawn, and the rows it had if it grows.
    drawn: Vec<(SessionKey, (usize, usize))>,
    /// Where an answer to a card may come from now, and since when.
    current: Option<card::Current>,
    /// Whether a prompt an agent opened is open, and when the last one closed.
    agents_prompt: bool,
    agents_prompt_closed: Option<Instant>,
    /// What the person writes in the minibuffer for a card, once `N` or `o` asked.
    writing: Option<Writing>,
    /// The plans being edited before they are approved (part 3d).
    plans: card::Plans,
}

impl App {
    /// Whether native session `key` asks the person now: a card shown, a plan's edit among
    /// them (part 3d).
    pub(super) fn native_asks_person(&self, key: SessionKey) -> bool {
        self.hosting.natives.permissions.asks.shown(key).is_some()
    }

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
            shown: None,
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

    /// Denies `ask`, whose session, bridge or socket went: its card goes, and its
    /// session no longer waits on it.
    fn drop_ask(&mut self, ask: Ask<Sender<Reply>>) {
        if ask.shown.is_some() {
            self.close_card(ask.key);
        }
        if ask.question.is_some() {
            self.call_waits(ask.key, &ask.call.tool_use_id, false);
            self.tell_tracker(ask.key, Target::Working);
        }
        let _ = (ask.reply).send(Ok(permission::deny(
            "Denied: oso could not ask the person.",
        )));
    }

    /// After each event, before the agent channel's questions, and after each batch of a
    /// native session's stream: finds the waiting requests' calls, denies those that cannot be
    /// asked, and shows each found one's card.
    pub(in crate::app) fn sync_permissions(&mut self) {
        self.note_agents_prompt();
        if self.hosting.natives.permissions.asks.is_empty() {
            return;
        }
        self.look_for_calls();
        self.show_cards();
        self.wake_for_permissions();
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
            match question(&session.name, tool, directory, ask.call.input.as_ref()) {
                Some(question) => Look::Found(question),
                None => Look::Unreadable,
            }
        });
        self.hosting.natives.permissions.asks = asks;
        if outcomes.iter().any(|o| matches!(o, Outcome::Found(_))) {
            self.say_calls_wait();
        }
        for outcome in outcomes {
            match outcome {
                // A session showing a card already waits for it: a later call waits behind it.
                Outcome::Found(key)
                    if self.hosting.natives.permissions.asks.shown(key).is_some() => {}
                Outcome::Found(key) => {
                    let tool = (self.hosting.natives.permissions.asks.unshown())
                        .find(|ask| ask.key == key)
                        .map(|ask| ask.call.tool_name.clone())
                        .unwrap_or_default();
                    self.tell_tracker(key, waits_for(&tool));
                }
                Outcome::Denied(ask, why, found) => {
                    if found {
                        self.call_waits(ask.key, &ask.call.tool_use_id, false);
                        self.tell_tracker(ask.key, Target::Working);
                    }
                    let _ = ask.reply.send(Ok(permission::deny(denial(why))));
                }
            }
        }
    }

    /// Each call found to wait for the person says so on its row.
    fn say_calls_wait(&mut self) {
        let asked: Vec<(SessionKey, String)> = (self.hosting.natives.permissions.asks.asked())
            .map(|(key, id)| (key, id.to_owned()))
            .collect();
        for (key, id) in asked {
            self.call_waits(key, &id, true);
        }
    }

    /// The row of session `key`'s call `id` says that it waits for the person, or no more.
    fn call_waits(&mut self, key: SessionKey, id: &str, waiting: bool) {
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        if let Some(change) = native.rows.waits(id, waiting) {
            super::draw(&mut self.editor, native, [change]);
            self.request_redraw();
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
}

/// The state a session waits in while its call's card is shown.
fn waits_for(tool: &str) -> Target {
    Target::NeedsInput(match tool {
        "AskUserQuestion" => Reason::Question,
        "ExitPlanMode" => Reason::Plan,
        _ => Reason::Permission,
    })
}

/// Whether the request's `input` says of the call what the stream's `tool` does: the same
/// target, as both are read the same way. A request with no input, as its input was too large
/// to send, says the same only of a call whose input the stream found too large too. An
/// `ExitPlanMode`'s stream carries no plan (measured): its id and tool are the match.
fn says_the_same(tool: &Tool, input: Option<&Value>) -> bool {
    if tool.name == "ExitPlanMode" {
        return input.is_some();
    }
    let Some(input) = input else {
        return tool.input.cut;
    };
    let sent = decode(&tool.name, Some(input));
    let kept = &tool.input;
    (sent.path == kept.path && sent.text == kept.text)
        && (sent.kind == kept.kind && sent.items == kept.items)
}

/// What the CLI is told of a request denied unasked.
fn denial(why: Denied) -> &'static str {
    match why {
        Denied::NotFound => "Denied: oso found no such call waiting in the session's stream.",
        Denied::NoLonger => "Denied: the call was no longer waiting when oso could ask.",
        Denied::Differs => {
            "Denied: the call oso was asked about is not the one the session's stream shows."
        }
        Denied::Unreadable => "Denied: oso could not read what the call asks the person.",
        Denied::Gone => "Denied: oso could not ask the person.",
    }
}

#[cfg(test)]
mod tests;
