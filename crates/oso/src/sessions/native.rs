//! Native sessions (ADR 0165): the person's own `claude -p` with stream-json in and out, shown
//! as oso rows instead of a terminal. This is step 1's pure part: what a line of the stream
//! says, the rows a session's buffer shows, and the state the stream gives. Nothing here
//! starts a process, reads a file or touches the UI.
//!
//! Must not depend on: `app`, `dispatch`, threads or the clock; plain functions and values
//! over the lines a caller gives.
//!
//! - `wire`: one line of the stream read into an [`wire::Event`], and the line that sends a
//!   message. The only place that touches JSON.
//! - `rows`: the events folded into a session's rows, and what changed with each event.
//! - `render`: a row as text and faces, folded or unfolded, as ADR 0130 has them.
//! - `turns`: what an event says of the session's state.

#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "step 1b's reader thread and buffer use it (ADR 0165)"
    )
)]

pub mod render;
pub mod rows;
pub mod turns;
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
