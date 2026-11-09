//! The worker's job (ADR 0082): a search to go on with and a count, over an O(1) snapshot of
//! a buffer's text, or the matches the keys that add cursors look for (ADR 0091), which the
//! host runs on its thread, and the report it sends back.
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
            Report::Found { generation, .. }
            | Report::Counted { generation, .. }
            | Report::Matches { generation, .. } => generation,
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
    /// What a search for cursors found (ADR 0091).
    Matches { generation: u64, matched: Matched },
}

/// The matches a search for cursors took, in the order it found them, and how many it
/// found in all, which may be more.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Matched {
    pub(crate) ranges: Vec<Range<usize>>,
    pub(crate) total: usize,
}

/// A search for cursors, over what it captured: `None` if `stop` said so first.
pub(crate) type MatchWork = Box<dyn FnOnce(&dyn Fn() -> bool) -> Option<Matched> + Send>;

/// What a job does.
enum Work {
    /// isearch's: go on with a search, then count.
    Isearch {
        text: Rope,
        invalid: InvalidBytes,
        query: Query,
        find: Option<Find>,
        /// Where the current match starts, if `find` does not say.
        current: Option<usize>,
    },
    /// The keys that add cursors': matches by rules of their own.
    Matches(MatchWork),
}

/// Work for a thread, over an O(1) snapshot of a buffer's text: a search to go on with, if
/// a key's budget ran out, and then a count. A newer job takes its place, and it stops
/// within a block once its generation is no longer the live one.
pub struct SearchJob {
    work: Work,
    generation: u64,
    live: Arc<AtomicU64>,
}

impl fmt::Debug for SearchJob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // No text and no offsets (ADR 0044).
        f.debug_struct("SearchJob")
            .field("generation", &self.generation)
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
            work: Work::Isearch {
                text,
                invalid,
                query,
                find,
                current,
            },
            generation,
            live,
        }
    }

    /// A job of `generation` that runs `work`, a search for cursors, while `live` holds it.
    pub(crate) fn matches(work: MatchWork, (generation, live): (u64, Arc<AtomicU64>)) -> Self {
        Self {
            work: Work::Matches(work),
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
    #[cfg(test)]
    pub(crate) fn superseded(&self) -> bool {
        self.live.load(Ordering::Relaxed) != self.generation
    }

    /// Does the work, and reports each part as it ends, unless superseded first.
    pub fn run(self, mut report: impl FnMut(SearchReport)) {
        let Self {
            work,
            generation,
            live,
        } = self;
        let stop = || live.load(Ordering::Relaxed) != generation;
        let (text, invalid, query, find, mut current) = match work {
            Work::Matches(work) => {
                if let Some(matched) = work(&stop) {
                    report(SearchReport(Report::Matches {
                        generation,
                        matched,
                    }));
                }
                return;
            }
            Work::Isearch {
                text,
                invalid,
                query,
                find,
                current,
            } => (text, invalid, query, find, current),
        };
        let (text, invalid, query) = (&text, &invalid, &query);
        if let Some(find) = find {
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
            report(SearchReport(Report::Found { generation, at }));
        }
        if let Some(count) = count(text, invalid, query, current, &stop) {
            report(SearchReport(Report::Counted {
                generation,
                count,
                at: current,
            }));
        }
    }
}
