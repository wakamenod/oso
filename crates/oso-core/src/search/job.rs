//! The worker's job (ADR 0082): a search to go on with and a count, over an O(1) snapshot of
//! a buffer's text, which the host runs on its thread, and the report it sends back.
//!
//! Must not depend on: `buffer`, `motion`, `editor`, threads or a log. Neither a job nor a
//! report says in `Debug` where anything is.

use std::fmt;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use super::{Count, Found, Query, count, find_backward, find_forward};
use crate::decode::InvalidBytes;
use crate::rope::Rope;

/// Where a worker goes on searching, as [`find_forward`](super::find_forward) and [`find_backward`](super::find_backward) take it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Find {
    Forward { from: usize },
    Backward { before: usize, start_at_most: usize },
}

/// What a worker found, for the editor to take in: the host only hands it back. `Debug`
/// gives no offsets, as they can say something of a buffer that may hold secrets.
#[derive(Clone, PartialEq, Eq)]
pub struct SearchReport(pub(crate) Report);

impl fmt::Debug for SearchReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let generation = match &self.0 {
            Report::Found { generation, .. } | Report::Counted { generation, .. } => generation,
        };
        f.debug_struct("SearchReport")
            .field("generation", generation)
            .finish_non_exhaustive()
    }
}

/// What a [`SearchReport`] says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Report {
    /// The search it went on with ended: at a match, or with none.
    Found {
        generation: u64,
        at: Option<Range<usize>>,
    },
    /// It counted the matches, for the match that starts at `at`.
    Counted {
        generation: u64,
        count: Count,
        at: Option<usize>,
    },
}

/// Work for a thread, over an O(1) snapshot of a buffer's text: a search to go on with, if
/// a key's budget ran out, and then a count. A newer job takes its place, and it stops
/// within a block once its generation is no longer the live one.
pub struct SearchJob {
    text: Rope,
    invalid: InvalidBytes,
    query: Query,
    find: Option<Find>,
    /// Where the current match starts, if `find` does not say.
    current: Option<usize>,
    generation: u64,
    live: Arc<AtomicU64>,
}

impl fmt::Debug for SearchJob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // No text and no offsets (ADR 0044).
        f.debug_struct("SearchJob")
            .field("generation", &self.generation)
            .field("finds", &self.find.is_some())
            .finish_non_exhaustive()
    }
}

impl SearchJob {
    /// A job of `generation`, which runs while `live` holds it.
    pub(crate) fn new(
        (text, invalid): (Rope, InvalidBytes),
        query: Query,
        find: Option<Find>,
        current: Option<usize>,
        (generation, live): (u64, Arc<AtomicU64>),
    ) -> Self {
        Self {
            text,
            invalid,
            query,
            find,
            current,
            generation,
            live,
        }
    }

    /// The job's generation, which its reports carry.
    #[cfg(test)]
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// Whether a newer job has taken its place, or the search has ended.
    pub(crate) fn superseded(&self) -> bool {
        self.live.load(Ordering::Relaxed) != self.generation
    }

    /// Does the work, and reports each part as it ends, unless superseded first.
    pub fn run(self, mut report: impl FnMut(SearchReport)) {
        let stop = || self.superseded();
        let (text, invalid, query) = (&self.text, &self.invalid, &self.query);
        let mut current = self.current;
        if let Some(find) = self.find {
            let found = match find {
                Find::Forward { from } => {
                    find_forward(text, invalid, query, from, usize::MAX, &stop)
                }
                Find::Backward {
                    before,
                    start_at_most,
                } => find_backward(
                    text,
                    invalid,
                    query,
                    before,
                    start_at_most,
                    usize::MAX,
                    &stop,
                ),
            };
            let at = match found {
                Found::Match(range) => Some(range),
                Found::None => None,
                Found::Stopped { .. } => return,
            };
            current = at.as_ref().map(|range| range.start);
            report(SearchReport(Report::Found {
                generation: self.generation,
                at,
            }));
        }
        if let Some(count) = count(text, invalid, query, current, &stop) {
            report(SearchReport(Report::Counted {
                generation: self.generation,
                count,
                at: current,
            }));
        }
    }
}
