//! Native sessions (ADR 0165): the person's own `claude -p` with stream-json in and out, shown
//! as oso rows instead of a terminal. This is step 1's pure part: what a line of the stream
//! says, the rows a session's buffer shows, and the state the stream gives. Nothing here
//! starts a process, reads a file or touches the UI.
//!
//! Must not depend on: `app`, `dispatch`, threads or the clock; plain functions and values
//! over the lines a caller gives.
//!
//! - `wire`: one line of the stream read into an [`wire::Event`], and the lines that send a
//!   message and a control request. The only place that touches JSON.
//! - `rows`: the events folded into a session's rows, and what changed with each event.
//! - `render`: a row as text and faces, folded or unfolded, as ADR 0130 has them, and
//!   `placed`: where each row is in the buffer.
//! - `point`: what the buffer's keys find at point: a row's call, the row a move goes to, and
//!   a call's whole result.
//! - `turns`: what an event says of the session's state.
//! - `card`: a permission card's text and an `AskUserQuestion`'s answers (step 2e-2).
//! - `asks`: the permission requests of the sessions' bridges, from their coming to their
//!   answer (step 2), at the times the caller gives.
//! - `queue`: the messages the person sent while a turn ran, held until the session is idle
//!   (step 2).
//! - `slash`: the slash commands a session's CLI offers, which run, and which are refused
//!   (step 2d).
//! - `control`: whether a session's CLI speaks the control layer, for which requests, and
//!   which request an answer is to; `interrupt`: an interrupt from its request to its
//!   fallback, a signal and a resume (step 2c).
//! - `markdown`: which replies are highlighted as Markdown, and their runs on their row
//!   (step 2, part 2g).
//! - `status`: the status line, model, mode, context, cost and rate limit (part 3a); `usage`:
//!   its `get_usage` request, once a process on tier 2.
//! - `suggest`: which prompt suggestion is kept, and its row; `recap`: the recap of a turn
//!   that ended out of sight (part 3f).

pub mod asks;
pub mod card;
pub mod control;
pub mod interrupt;
pub mod markdown;
pub mod placed;
pub mod point;
pub mod queue;
pub mod recap;
pub mod render;
pub mod rows;
pub mod slash;
pub mod status;
pub mod suggest;
pub mod turns;
pub mod usage;
pub mod wire;

#[cfg(test)]
mod fuzz_tests;
#[cfg(test)]
mod tests;

/// Cuts `text` to at most `max` bytes, at a character's start.
pub(crate) fn capped(text: &str, max: usize) -> (&str, bool) {
    if text.len() <= max {
        return (text, false);
    }
    (&text[..text.floor_char_boundary(max)], true)
}
