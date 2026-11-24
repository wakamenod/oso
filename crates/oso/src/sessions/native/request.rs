//! A native session's request in its conversation (ADR 0182): what a call that waits for the
//! person shows, as text and faces, the line that says its keys, and what of it has been read.
//! A call's only: questions and plans keep their cards until their steps.
//!
//! Must not depend on: threads, the stream or the UI; the caller gives what the call says, the
//! widths, what its window shows and the times, and puts the text in the session's buffer.
//!
//! - `body`: the text under the title, made once, off the UI thread, as a change is compared.
//! - `title`: the words on the title's band, the only text of a block drawn again.
//! - `line`: the text of the dock that says the keys.
//! - `reading`: what of the body was shown long enough, as ranges of its bytes.

pub mod body;
pub mod line;
pub mod reading;
mod title;

#[cfg(test)]
mod fuzz_tests;
#[cfg(test)]
mod measure_tests;
#[cfg(test)]
mod tests;

use std::time::{Duration, Instant};

use serde_json::Value;

use self::body::{Count, Hidden};
use super::render::Rendered;
use super::wire::request::{changes_a_file, parts};

/// How long a call's changes may be compared, all of them together, off the UI thread.
pub const COMPARE_WITHIN: Duration = Duration::from_millis(50);

/// The block of a call that waits: its text under the title, which never changes, and what its
/// title and its line say of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    tool: String,
    changes: bool,
    count: Option<Count>,
    body: Rendered,
    hidden: Hidden,
}

impl Block {
    /// The block of a call of `tool`, one that is neither a question nor a plan, with the `input`
    /// its request carries. What a change is compared by runs until `deadline`, past which it is shown
    /// as all of the old text, then all of the new: not for the UI thread.
    pub fn call(tool: &str, input: Option<&Value>, deadline: Instant) -> Self {
        let parts = input.and_then(|input| parts(tool, input));
        let whole = title::whole_name(tool);
        let name = (!title::known(tool)).then_some(whole.as_str());
        let (body, count, hidden) = match parts {
            Some(parts) => body::body(name, &parts, deadline),
            None => body::not_shown(name),
        };
        Self {
            tool: title::short(&whole),
            changes: changes_a_file(tool),
            count,
            body,
            hidden,
        }
    }

    /// Its text under the title and its faces: what is read and what keeps its place are counted
    /// in its bytes.
    pub fn body(&self) -> &Rendered {
        &self.body
    }

    /// What of the call it does not show.
    pub fn hidden(&self) -> Hidden {
        self.hidden
    }

    /// Its title, one row with its line break and no blank one before it, `width` cells wide, as request `place.0` of
    /// `place.1` that wait.
    pub fn title(&self, place: (usize, usize), width: usize) -> Rendered {
        title::title(self, place, width)
    }

    /// What it asks, in oso's words after the tool's name.
    fn asks(&self) -> &'static str {
        if self.changes {
            "asks to change a file"
        } else {
            "asks to run"
        }
    }
}
