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
//! - `point`: what the buffer's keys find at point: a row's call, the row a move goes to, a
//!   reply's Markdown and a call's whole result.
//! - `turns`: what an event says of the session's state.
//! - `card`: a permission card's text and an `AskUserQuestion`'s answers (step 2e-2).
//! - `request`: a request's block, the line of its keys and what of it was read (ADR 0182); a
//!   change in it is compared until the time the caller gives.
//! - `asks`: the permission requests of the sessions' bridges, from their coming to their
//!   answer (step 2), at the times the caller gives.
//! - `queue`: the messages the person sent while a turn ran, held until the session is idle
//!   (step 2).
//! - `slash`: the slash commands a session's CLI offers, which run, and which are refused
//!   (step 2d).
//! - `control`: whether a session's CLI speaks the control layer, for which requests, and
//!   which request an answer is to; `interrupt`: an interrupt from its request to its
//!   fallback, a signal and a resume (step 2c).
//! - `shown`: a reply's Markdown as it is drawn, its marks hidden, a line at a time.
//! - `markdown`: which replies are highlighted as Markdown, and their runs on their row
//!   (step 2, part 2g).
//! - `status`: the status line, model, mode, context, cost and rate limit (part 3a); `usage`:
//!   its `get_usage` request, once a process on tier 2.
//! - `suggest`: which prompt suggestion is kept; `recap`: the recap of a turn
//!   that ended out of sight (part 3f).
//! - `mode`: the permission mode the CLI says, and the person's switch of it, from its request
//!   to its fallback, a resume in the new mode (part 3b).
//! - `todo`: the session's to-do list from its to-do tools' results, and the strip that
//!   shows it (step 3, part 3e).
//! - `choice`: the model and effort level the person chose with `/model` and `/effort`, once
//!   the CLI says it took them (part 3c).
//! - `switch`: `session-switch-mode`'s rules, and where each switch between native and the
//!   TUI is, from the end of one mode's process to the other's start (part 4d).
//! - `remote`: where Remote Control is in a session's process, and which messages came another
//!   way than oso's standard input (ADR 0181).
//! - `past`: which lines of a session's transcript its earlier turns show, and what is left
//!   out (step 5).

pub mod asks;
pub mod card;
pub mod choice;
pub mod control;
pub mod interrupt;
pub mod markdown;
pub mod mode;
pub mod past;
pub mod placed;
pub mod point;
pub mod queue;
pub mod recap;
pub mod remote;
pub mod render;
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "nothing shows a request until ADR 0182's R1c")
)]
pub mod request;
pub mod rows;
pub mod shown;
pub mod slash;
pub mod status;
pub mod suggest;
pub mod switch;
pub mod todo;
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
