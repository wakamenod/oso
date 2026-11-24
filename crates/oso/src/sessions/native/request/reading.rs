//! What of a request was read (ADR 0182, "Reading"): which rows of its block its window showed
//! whole, unmoved, for long enough, kept as ranges of the body's bytes.
//!
//! Must not depend on: the app, the editor or the clock; the caller says what its window shows,
//! whether a frame of that was presented, and the time.
//!
//! - A row is read once it was in view for [`READ_AFTER`], in a view that was presented, was
//!   attended and did not change meanwhile: a look that finds another view starts the wait again.
//! - A row is in view as [`rows::rows`] gives it from what is laid out: whole, and not clipped, as
//!   one of a wrap not made yet is, wider than its window.
//! - What was read is never taken back, and is the block's: a new block has a new [`Reading`].
//! - A block that does not show all of its call is never all read, whatever was.

pub mod rows;

use std::ops::Range;
use std::time::{Duration, Instant};

use oso_core::window::WindowId;

use super::Block;
use super::body::Hidden;

/// How long a row is in view until it is read.
pub const READ_AFTER: Duration = Duration::from_millis(500);

/// A row of the body in view, as it is laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    /// Its first byte of the body, and where the next row starts: a line's last row has its break.
    pub start: usize,
    pub end: usize,
    /// Which row of its window it is, from the top.
    pub at: usize,
    /// The cells it takes from the window's left edge, its indent among them.
    pub cells: usize,
}

/// What the window that carries the session's docks shows of the body.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct View {
    /// The window.
    pub window: Option<WindowId>,
    /// The cells of its text area. A row wider than that is clipped, as one of a wrap not made
    /// yet is.
    pub columns: usize,
    /// Its rows are wrapped, and not scrolled sideways.
    pub wraps: bool,
    /// The window or one of its docks is selected, oso's window is in front and in sight, and no
    /// prompt, menu or float is open.
    pub attended: bool,
    /// The body's rows in view, whole, in order.
    pub rows: Vec<Row>,
}

impl View {
    /// The bytes, of a body of `len`, of its rows that can be read where it shows them.
    fn readable(&self, len: usize) -> impl Iterator<Item = Range<usize>> {
        (self.rows.iter())
            .filter(|row| self.wraps && row.cells <= self.columns)
            .map(move |row| row.start..row.end.min(len))
            .filter(|bytes| !bytes.is_empty())
    }
}

/// What of a block was read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reading {
    /// Where each line of the body ends, past its break.
    ends: Vec<usize>,
    hidden: Hidden,
    /// The bytes read: in order, apart, and none empty.
    read: Vec<Range<usize>>,
    /// What is in view, whether or not it can be read now.
    view: View,
    /// Since when a frame of that was presented, all the while attended.
    since: Option<Instant>,
    /// What is in view was read already.
    settled: bool,
    /// Some of it was in view for long enough.
    seen: bool,
}

impl Reading {
    /// Nothing of `block` read.
    pub fn new(block: &Block) -> Self {
        let text = &block.body.text;
        Self {
            ends: text.match_indices('\n').map(|(at, _)| at + 1).collect(),
            hidden: block.hidden,
            read: Vec::new(),
            view: View::default(),
            since: None,
            settled: false,
            seen: false,
        }
    }

    /// Looks at `view` at `now`, after each event while the request waits, before a key answers,
    /// and as each frame was `presented`. Whether what was read, or may be, changed.
    pub fn look(&mut self, view: &View, presented: bool, now: Instant) -> bool {
        let changed = self.view != *view;
        if changed {
            self.view.rows.clear();
            self.view.rows.extend_from_slice(&view.rows);
            self.view.window = view.window;
            self.view.columns = view.columns;
            self.view.wraps = view.wraps;
            self.view.attended = view.attended;
            // What it showed is gone, whether or not a frame of what it shows was presented.
            self.since = None;
            self.settled = false;
        }
        if self.settled || !view.attended || !view.wraps {
            return changed;
        }
        match self.since {
            Some(since) if now.saturating_duration_since(since) >= READ_AFTER => {
                self.settled = true;
                self.take() || changed
            }
            Some(_) => changed,
            None => {
                self.since = presented.then_some(now);
                changed || presented
            }
        }
    }

    /// When what is in view is read, unless something changes first: look again then.
    pub fn due(&self) -> Option<Instant> {
        let since = self.since.filter(|_| !self.settled)?;
        let more = !self.view.rows.is_empty()
            && (!self.seen || self.readable().any(|bytes| !self.covers(&bytes)));
        more.then(|| since + READ_AFTER)
    }

    /// The bytes of the rows in view that can be read.
    fn readable(&self) -> impl Iterator<Item = Range<usize>> {
        self.view.readable(self.len())
    }

    /// The rows in view that can be read are.
    fn take(&mut self) -> bool {
        let readable: Vec<Range<usize>> = self.readable().collect();
        let mut more = !self.seen && !readable.is_empty();
        self.seen |= more;
        for bytes in readable {
            if self.covers(&bytes) {
                continue;
            }
            more = true;
            // Those it meets become one with it.
            let from = self.read.partition_point(|read| read.end < bytes.start);
            let to = self.read.partition_point(|read| read.start <= bytes.end);
            let start =
                (self.read[from..to].first()).map_or(bytes.start, |r| r.start.min(bytes.start));
            let end = (self.read[from..to].last()).map_or(bytes.end, |r| r.end.max(bytes.end));
            self.read.splice(from..to, std::iter::once(start..end));
        }
        more
    }

    fn len(&self) -> usize {
        self.ends.last().copied().unwrap_or(0)
    }

    /// Whether all of `bytes` was read.
    fn covers(&self, bytes: &Range<usize>) -> bool {
        let at = self.read.partition_point(|read| read.end < bytes.end);
        (self.read.get(at)).is_some_and(|read| read.start <= bytes.start && bytes.end <= read.end)
    }

    /// What of the call the block does not show.
    pub fn hidden(&self) -> Hidden {
        self.hidden
    }

    /// Whether the whole call was read: never one its block does not show all of.
    pub fn all_read(&self) -> bool {
        self.hidden == Hidden::Nothing && self.shown_read()
    }

    /// Whether all the block shows was read.
    pub fn shown_read(&self) -> bool {
        self.unread().is_none()
    }

    /// The first byte of the body not read.
    pub fn unread(&self) -> Option<usize> {
        let first = match self.read.first() {
            Some(read) if read.start == 0 => read.end,
            _ => 0,
        };
        (first < self.len()).then_some(first)
    }

    /// How many of the body's lines were read whole, and how many it has, those it does not show
    /// among them.
    pub fn lines(&self) -> (usize, usize) {
        let (shown, more) = match self.hidden {
            Hidden::Nothing => (self.ends.len(), 0),
            // The last line says how many are not shown, as the only one of none shown does.
            Hidden::Lines(lines) => (self.ends.len().saturating_sub(1), lines),
            Hidden::All => (0, 0),
        };
        let mut start = 0;
        let mut read = 0;
        for &end in self.ends.iter().take(shown) {
            read += usize::from(self.covers(&(start..end)));
            start = end;
        }
        (read, shown.saturating_add(more))
    }

    /// Whether `y` allows the call, the wait aside: all of it was read, and a frame of what is in
    /// view, its last row in it, was presented and is attended. Look first.
    pub fn allows(&self) -> bool {
        self.all_read() && self.since.is_some() && self.last_in_view()
    }

    /// Whether `C-u y` may ask about the rest, the wait aside: some of the block was in view for
    /// long enough.
    pub fn may_ask(&self) -> bool {
        self.seen
    }

    /// Whether the body's last row is in view, where it can be read.
    pub fn last_in_view(&self) -> bool {
        let len = self.len();
        self.readable().any(|bytes| bytes.end >= len)
    }

    /// Whether all that is left to read is in view, where it can be.
    pub fn unread_in_view(&self) -> bool {
        // Where what was read, or will be, from the first byte on ends: rows are in order.
        let past = |at: usize| {
            let next = self.read.partition_point(|read| read.end <= at);
            (self.read.get(next))
                .filter(|read| read.start <= at)
                .map_or(at, |read| read.end)
        };
        let mut at = past(0);
        for bytes in self.readable() {
            if bytes.contains(&at) {
                at = past(bytes.end);
            }
        }
        at >= self.len()
    }
}

#[cfg(test)]
mod rows_tests;
#[cfg(test)]
mod tests;
