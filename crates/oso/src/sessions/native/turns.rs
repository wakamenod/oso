//! What a line of a native session's stream says of its state (ADR 0165, "What oso reuses"):
//! the stream is the authority, so no screen is read.
//!
//! Must not depend on: threads or the clock; the caller gives each [`Target`] to the session's
//! tracker (`state`) with its own instant.
//!
//! - A turn starts when oso writes a message, which the caller knows; the CLI's replay of it
//!   says so again, and a request or a compaction says the CLI works.
//! - A turn ends with its `result`, whatever it says: an error shows in its result row, and a
//!   session is `failed` only when its process ends as oso did not ask (ADR 0165), which is
//!   the caller's to see, as for a terminal.
//! - The tracker as ADR 0121 has it holds back what a new session's screen says for a grace
//!   period and debounces idleness; a stream needs neither, which step 1b's entry for it says.

use super::wire::{Event, Status, TurnResult};
use crate::sessions::state::Target;

/// What `event` says of the session's state, if anything.
pub fn target(event: &Event) -> Option<Target> {
    match event {
        Event::User(user) if user.replay && !user.synthetic && user.parent.is_none() => {
            Some(Target::Working)
        }
        Event::Status(Status::Requesting | Status::Compacting) => Some(Target::Working),
        Event::Result(_) => Some(Target::Idle { visible: true }),
        _ => None,
    }
}

/// Whether the turn ended because it was interrupted.
pub fn interrupted(result: &TurnResult) -> bool {
    result
        .reason
        .as_deref()
        .is_some_and(|reason| reason.starts_with("aborted"))
}
