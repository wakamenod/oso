//! The reply to a channel's `undo` or `undo-redo` whose step goes on over turns (ADR 0119): it
//! waits until the undo or the redo has ended, or the person cancelled it, and says which.
//!
//! Must not depend on: sockets or threads, which stay behind the channel's requests.

use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use oso_core::buffer::EditError;

use super::super::App;
use crate::channel::server::ANSWER_WITHIN;
use crate::channel::{Connection, Reply};

/// How long a reply waits for the undo, under the channel's wait for a reply: then it says the
/// undo goes on, and `state`'s `undoing` says how far.
const UNDO_WITHIN: Duration = ANSWER_WITHIN;

/// A request whose reply waits until the undo or the redo its command began has ended.
#[derive(Debug)]
pub(super) struct Waiting {
    connection: Connection,
    reply: Sender<Reply>,
    /// What the command's reply says, once the undo ends.
    ran: Reply,
    redo: bool,
    started: Instant,
}

impl App {
    /// Holds back the reply `ran` of a command that left an undo or a redo under way, until it
    /// ends. Returns the reply if none is under way, to send at once.
    pub(super) fn wait_for_undo(
        &mut self,
        connection: Connection,
        reply: Sender<Reply>,
        ran: Reply,
    ) -> Option<(Sender<Reply>, Reply)> {
        let Some(redo) = self.editor.crossing().map(oso_core::editor::Crossing::redo) else {
            return Some((reply, ran));
        };
        self.agent.undoing = Some(Waiting {
            connection,
            reply,
            ran,
            redo,
            started: Instant::now(),
        });
        self.wake_after(UNDO_WITHIN);
        None
    }

    /// After a turn of the undo under way: replies to the request that began it once it has
    /// ended, as `ended` says, or `C-g` cancelled it; past [`UNDO_WITHIN`], says it goes on.
    pub(in crate::app) fn agent_undo_turn(
        &mut self,
        ended: Option<Result<(), EditError>>,
        cancelled: bool,
    ) {
        let Some(waiting) = &self.agent.undoing else {
            return;
        };
        let what = if waiting.redo { "redo" } else { "undo" };
        let (outcome, reply) = match ended {
            _ if cancelled => {
                let cancelled = if waiting.redo {
                    "Redo cancelled by the person"
                } else {
                    "Undo cancelled by the person"
                };
                ("cancelled", Err(cancelled.to_owned()))
            }
            Some(Ok(())) => ("ended", waiting.ran.clone()),
            Some(Err(error)) => ("failed", Err(error.to_string())),
            None if waiting.started.elapsed() >= UNDO_WITHIN => (
                "goes on past the reply",
                Ok(serde_json::json!({
                    "waiting": format!("the {what} goes on; state's `undoing` says how far"),
                })),
            ),
            None => return,
        };
        self.agent_log(&format!("{what}: {outcome}"));
        if let Some(waiting) = self.agent.undoing.take() {
            let _ = waiting.reply.send(reply);
        }
    }

    /// Forgets the reply `connection` waits for, if any: it goes nowhere now, and the undo
    /// goes on.
    pub(super) fn forget_undo_of(&mut self, connection: Option<Connection>) {
        if self
            .agent
            .undoing
            .as_ref()
            .is_some_and(|waiting| connection.is_none_or(|gone| waiting.connection == gone))
        {
            self.agent.undoing = None;
        }
    }
}
