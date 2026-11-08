//! What an isearch holds (ADR 0082): the string, where it has got to, the count, the steps
//! `DEL` goes back through, and the echo line it says.
//!
//! Must not depend on: `command` or `keymap`, or the host's window or threads.
//!
//! `Debug` prints none of the string, which can say something of a buffer that may hold
//! secrets (ADR 0044).

use std::fmt::{self, Write};
use std::ops::Range;

use crate::buffer::BufferId;
use crate::search::{self, MAX_QUERY_CHARS, Query};
use crate::selection::SelectionSet;
use crate::window::Scroll;

/// The most steps `DEL` can go back. The oldest go first.
pub(super) const MAX_STEPS: usize = 4096;

/// What isearch knows of the count of matches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tally {
    /// A worker is counting. `moves` says how many matches the current one has moved by
    /// since the match it counts for.
    Counting { moves: isize },
    /// The current match is the `index`th of `total`, from 1, or 0 with none.
    Known { index: usize, total: usize },
}

/// How a search was asked for, which says how the count follows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum How {
    /// The string, the case or the text changed: the count starts again.
    Typed,
    /// A repeat, to the next match or, wrapping, round to the first.
    Repeat { wrapped: bool },
}

/// What `DEL` goes back to.
#[derive(Clone, Debug)]
pub(super) struct Step {
    pub(super) len: usize,
    pub(super) backward: bool,
    pub(super) flipped: bool,
    pub(super) current: Option<Range<usize>>,
    pub(super) failing: bool,
    pub(super) wrapped: bool,
    pub(super) cursor: usize,
    pub(super) tally: Option<Tally>,
}

/// An isearch under way in the selected window.
pub struct Isearch {
    pub(super) string: String,
    pub(super) backward: bool,
    /// `M-c` turned smart case around.
    pub(super) flipped: bool,
    pub(super) query: Option<Query>,
    /// The match the cursor is at. While failing, the last that matched, of a shorter string.
    pub(super) current: Option<Range<usize>>,
    pub(super) failing: bool,
    pub(super) wrapped: bool,
    /// A worker goes on with the search, and asked as this.
    pub(super) searching: Option<How>,
    pub(super) tally: Option<Tally>,
    /// The buffer and the cursors, marks and view before it began, for `C-g`.
    pub(super) buffer: BufferId,
    pub(super) begin: (SelectionSet, Scroll),
    pub(super) revision: u64,
    pub(super) steps: Vec<Step>,
    /// It began, or went on, while a buffer that may hold secrets was shown.
    pub(super) around_secret: bool,
    /// Where `M-p` and `M-n` are in the history: 0 the newest.
    pub(super) history: Option<usize>,
}

impl fmt::Debug for Isearch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The string stays out of it (ADR 0044).
        f.debug_struct("Isearch")
            .field("backward", &self.backward)
            .field("failing", &self.failing)
            .field("wrapped", &self.wrapped)
            .finish_non_exhaustive()
    }
}

impl Isearch {
    /// The search string.
    pub fn string(&self) -> &str {
        &self.string
    }

    /// Whether it searches backward.
    pub fn backward(&self) -> bool {
        self.backward
    }

    /// Whether the string has no match where it searched.
    pub fn failing(&self) -> bool {
        self.failing
    }

    /// Whether it went round the buffer's end.
    pub fn wrapped(&self) -> bool {
        self.wrapped
    }

    /// Whether case matters: smart case, turned around by `M-c`.
    pub fn case_sensitive(&self) -> bool {
        search::smart_case(&self.string) != self.flipped
    }

    /// Whether a worker goes on with the search.
    pub fn searching(&self) -> bool {
        self.searching.is_some()
    }

    /// What is known of the count, `None` with no string.
    pub fn tally(&self) -> Option<Tally> {
        self.tally
    }

    /// Whether it began, or went on, while a buffer that may hold secrets was shown, so
    /// that the control socket leaves it out and its string enters no history.
    pub fn around_secret(&self) -> bool {
        self.around_secret
    }

    /// Marks it as around a buffer that may hold secrets.
    pub(in crate::editor) fn note_secret(&mut self) {
        self.around_secret = true;
    }

    /// The echo line: `I-search: foo [3/17]`, with Emacs's words before it.
    pub(super) fn write_line(&self, out: &mut String) {
        let mut words: Vec<&str> = Vec::new();
        if self.failing {
            words.push("failing");
        }
        if self.wrapped {
            words.push("wrapped");
        }
        if self.flipped {
            words.push(if self.case_sensitive() {
                "case-sensitive"
            } else {
                "case-insensitive"
            });
        }
        words.push("I-search");
        if self.backward {
            words.push("backward");
        }
        let start = out.len();
        out.push_str(&words.join(" "));
        if let Some(first) = out[start..].chars().next() {
            let upper: String = first.to_uppercase().collect();
            out.replace_range(start..start + first.len_utf8(), &upper);
        }
        out.push_str(": ");
        out.push_str(&self.string);
        if self.searching.is_some() {
            out.push_str(" [searching…]");
            return;
        }
        let _ = match self.tally {
            None => Ok(()),
            Some(Tally::Counting { .. }) => write!(out, " [?/…]"),
            Some(Tally::Known { index, total }) => write!(out, " [{index}/{total}]"),
        };
    }

    pub(super) fn step(&self, cursor: usize) -> Step {
        Step {
            len: self.string.len(),
            backward: self.backward,
            flipped: self.flipped,
            current: self.current.clone(),
            failing: self.failing,
            wrapped: self.wrapped,
            cursor,
            tally: self.tally,
        }
    }

    pub(super) fn push_step(&mut self, cursor: usize) {
        if self.steps.len() == MAX_STEPS {
            self.steps.remove(0);
        }
        let step = self.step(cursor);
        self.steps.push(step);
    }

    pub(super) fn rebuild_query(&mut self) {
        self.query = Query::new(&self.string, self.case_sensitive());
    }

    /// Adds `text` up to [`MAX_QUERY_CHARS`]. Returns whether any went in.
    pub(super) fn add(&mut self, text: &str) -> bool {
        let room = MAX_QUERY_CHARS.saturating_sub(self.string.chars().count());
        let end = text
            .char_indices()
            .nth(room)
            .map_or(text.len(), |(at, _)| at);
        self.string.push_str(&text[..end]);
        end > 0
    }
}
