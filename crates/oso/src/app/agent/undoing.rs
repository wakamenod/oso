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
use crate::dispatch::Cancelled;

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
    /// The steps the command asked to cross.
    steps: usize,
    started: Instant,
}

impl App {
    /// Holds back the reply `ran` of a command that left an undo or a redo of `steps` steps
    /// under way, until it ends. Returns the reply if none is under way, to send at once.
    pub(super) fn wait_for_undo(
        &mut self,
        connection: Connection,
        reply: Sender<Reply>,
        (ran, steps): (Reply, usize),
    ) -> Option<(Sender<Reply>, Reply)> {
        let Some(redo) = self.editor.crossing().map(oso_core::editor::Crossing::redo) else {
            return Some((reply, ran));
        };
        // The channel waits while an undo goes on, so no reply waits already; were one to, it
        // hears that it went unanswered rather than nothing.
        if let Some(earlier) = self.agent.undoing.take() {
            log::warn!("agent channel: an undo's reply waited when another began");
            let _ = earlier.reply.send(Err(
                "another undo began before this one's end was known".into()
            ));
        }
        self.agent.undoing = Some(Waiting {
            connection,
            reply,
            ran,
            redo,
            steps,
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
        cancelled: Option<Cancelled>,
    ) {
        let Some(waiting) = &self.agent.undoing else {
            return;
        };
        let what = if waiting.redo { "redo" } else { "undo" };
        let (outcome, reply) = match (ended, cancelled) {
            (_, Some(Cancelled::Crossing { left })) => (
                "cancelled",
                Err(cancelled_reply(waiting.redo, waiting.steps, left)),
            ),
            (Some(Ok(())), _) => ("ended", waiting.ran.clone()),
            (Some(Err(error)), _) => ("failed", Err(error.to_string())),
            (None, _) if waiting.started.elapsed() >= UNDO_WITHIN => (
                "goes on past the reply",
                Ok(serde_json::json!({
                    "waiting": format!("the {what} goes on; state's `undoing` says how far"),
                })),
            ),
            (None, _) => return,
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

/// What the agent hears when the person cancels its undo or redo of `steps` steps with `left`
/// more after the one cancelled: with a count, how many crossed, which stay crossed.
fn cancelled_reply(redo: bool, steps: usize, left: usize) -> String {
    let what = if redo { "Redo" } else { "Undo" };
    if steps > 1 {
        let crossed = steps.saturating_sub(left + 1);
        format!("{what} cancelled by the person after {crossed} of {steps} steps")
    } else {
        format!("{what} cancelled by the person")
    }
}

#[cfg(test)]
mod tests {
    use super::cancelled_reply;

    #[test]
    fn a_cancelled_count_says_how_many_steps_crossed() {
        assert_eq!(cancelled_reply(false, 1, 0), "Undo cancelled by the person");
        assert_eq!(
            cancelled_reply(true, 5, 2),
            "Redo cancelled by the person after 2 of 5 steps"
        );
    }
}
