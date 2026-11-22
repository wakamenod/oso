//! The rows of a native session's queue (ADR 0165, step 2): a heading that says how many wait
//! and why none goes, if so, then each message's first line, cut, the one edited marked.
//!
//! Must not depend on: the app; it reads the queue's state and writes text.

use std::fmt::Write as _;

use super::one_line;
use crate::sessions::native::queue::{Pause, Queue};

/// The queue's rows; nothing while none waits.
pub fn queue_rows(queue: &Queue) -> String {
    let mut text = String::new();
    let count = queue.waiting().len();
    if count == 0 {
        return text;
    }
    let paused = match queue.paused() {
        None => "",
        Some(Pause::Failed) => " · paused: C-c C-c on an empty composer sends the next",
        Some(Pause::Ended) => " · the session stopped: RET takes one back, d drops it",
    };
    let _ = writeln!(text, "Queued ({count}){paused}");
    for (at, queued) in queue.waiting().enumerate() {
        let first = queued.text.lines().next().unwrap_or("");
        let mut shown = one_line(first);
        if first.len() < queued.text.trim_end().len() && !shown.ends_with('…') {
            shown.push('…');
        }
        let editing = if queue.editing() == Some(queued.id) {
            " (editing)"
        } else {
            ""
        };
        let _ = writeln!(text, "  {}. {shown}{editing}", at + 1);
    }
    text
}
