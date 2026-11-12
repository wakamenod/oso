//! The worker's job (ADR 0082): a search to go on with and a count, over an O(1) snapshot of
//! a buffer's text, after compiling a regexp too large for a key (ADR 0116), or the matches
//! the keys that add cursors look for (ADR 0091), which the host runs on its thread, and the
//! report it sends back.
//!
//! Must not depend on: `buffer`, `motion`, `editor`, threads or a log. Neither a job nor a
//! report says in `Debug` where anything is.

use std::fmt;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use super::regexp::{self, Regexp};
use super::{Count, Found, Limits, LineTooLong, Needle};
use crate::decode::InvalidBytes;
use crate::rope::Rope;

/// Where a worker goes on searching, as [`Needle::find_forward`] and
/// [`Needle::find_backward`] take it: forward, with an empty match at `from` skipped if a
/// match ended there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Find {
    Forward { from: usize, after_match: bool },
    Backward { before: usize, start_at_most: usize },
}

/// What an isearch job looks for: a needle, or a regexp's pattern too large to compile on
/// the UI thread, which the worker compiles first (ADR 0116).
#[derive(Clone)]
pub(crate) enum Prepared {
    Ready(Needle),
    Compile { pattern: String, insensitive: bool },
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
            | Report::Matches { generation, .. }
            | Report::LineTooLong { generation, .. }
            | Report::Compiled { generation, .. } => generation,
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
    /// A regexp met a line too long to match in, which starts at `at` (ADR 0113, ADR 0116).
    LineTooLong { generation: u64, at: usize },
    /// The worker compiled an isearch's regexp, or says what is wrong with it.
    Compiled {
        generation: u64,
        regexp: Result<Regexp, String>,
    },
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
    /// isearch's: compile if need be, go on with a search, then count.
    Isearch {
        text: Rope,
        invalid: InvalidBytes,
        needle: Prepared,
        find: Option<Find>,
        /// Where the current match starts, if `find` does not say.
        current: Option<usize>,
    },
    /// The keys that add cursors': matches by rules of their own.
    Matches(MatchWork),
    /// query-replace's (ADR 0113): within its ranges, the next match from `find`, then a
    /// count of the scope as `count` holds it, if given.
    Scoped {
        text: Rope,
        invalid: InvalidBytes,
        needle: Needle,
        ranges: Vec<Range<usize>>,
        find: Option<(usize, bool)>,
        count: Option<Counting>,
    },
}

/// The scope a query-replace counts: its text and ranges when the session began, so that a
/// count started after replacements counts what was there to replace (ADR 0113).
#[derive(Clone)]
pub(crate) struct Counting {
    pub(crate) text: Rope,
    pub(crate) invalid: InvalidBytes,
    pub(crate) ranges: Vec<Range<usize>>,
}

/// Where a search goes on after `found`: its end, or past an empty match, the next
/// character's start.
pub(crate) fn after(text: &Rope, found: &Range<usize>) -> usize {
    if found.is_empty() && found.end >= text.len() {
        // Past the end, so that the same empty match is not found again.
        found.end + 1
    } else if found.is_empty() {
        text.clip_offset(found.end + 1, crate::sum_tree::Bias::Right)
    } else {
        found.end
    }
}

/// Does a scoped job's work, and reports each part as it ends, unless `stop` says so first.
fn run_scoped(
    snapshot: (&Rope, &InvalidBytes),
    needle: &Needle,
    ranges: &[Range<usize>],
    (find, count): (Option<(usize, bool)>, Option<&Counting>),
    stop: &dyn Fn() -> bool,
    generation: u64,
    report: &mut dyn FnMut(Report),
) {
    if let Some(from) = find {
        let mut scan = needle.scan(snapshot, ranges, from, usize::MAX, None);
        match scan.next(stop) {
            Ok(Found::Match(at)) => report(Report::Found {
                generation,
                at: Some(at),
            }),
            Ok(Found::None) => report(Report::Found {
                generation,
                at: None,
            }),
            Ok(Found::Stopped { .. }) => return,
            Err(LineTooLong(at)) => {
                report(Report::LineTooLong { generation, at });
                return;
            }
        }
    }
    let Some(counting) = count else {
        return;
    };
    let snapshot = (&counting.text, &counting.invalid);
    let mut scan = needle.scan(snapshot, &counting.ranges, (0, false), usize::MAX, None);
    let mut total = 0;
    loop {
        match scan.next(stop) {
            Ok(Found::Match(_)) => total += 1,
            Ok(Found::None) => break,
            // Stopped, or past a line too long, which would say less than there is.
            Ok(Found::Stopped { .. }) | Err(_) => return,
        }
    }
    report(Report::Counted {
        generation,
        count: Count { total, before: 0 },
        at: None,
    });
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
        needle: Prepared,
        find: Option<Find>,
        current: Option<usize>,
        (generation, live): (u64, Arc<AtomicU64>),
    ) -> Self {
        Self {
            work: Work::Isearch {
                text,
                invalid,
                needle,
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

    /// A job of `generation` for query-replace, over `ranges` of the snapshot: the next match
    /// from `find`, then a count of `count`, while `live` holds it.
    pub(crate) fn scoped(
        (text, invalid): (Rope, InvalidBytes),
        needle: Needle,
        ranges: Vec<Range<usize>>,
        (find, count): (Option<(usize, bool)>, Option<Counting>),
        (generation, live): (u64, Arc<AtomicU64>),
    ) -> Self {
        Self {
            work: Work::Scoped {
                text,
                invalid,
                needle,
                ranges,
                find,
                count,
            },
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
        let (text, invalid, needle, find, current) = match work {
            Work::Matches(work) => {
                if let Some(matched) = work(&stop) {
                    report(SearchReport(Report::Matches {
                        generation,
                        matched,
                    }));
                }
                return;
            }
            Work::Scoped {
                text,
                invalid,
                needle,
                ranges,
                find,
                count,
            } => {
                let done = &mut |done| report(SearchReport(done));
                let snapshot = (&text, &invalid);
                let what = (find, count.as_ref());
                run_scoped(snapshot, &needle, &ranges, what, &stop, generation, done);
                return;
            }
            Work::Isearch {
                text,
                invalid,
                needle,
                find,
                current,
            } => (text, invalid, needle, find, current),
        };
        let needle = match needle {
            Prepared::Ready(needle) => needle,
            Prepared::Compile {
                pattern,
                insensitive,
            } => {
                let compiled = compile_on_worker(&pattern, insensitive, &stop);
                let Some(compiled) = compiled else {
                    return;
                };
                report(SearchReport(Report::Compiled {
                    generation,
                    regexp: compiled.clone(),
                }));
                let Ok(regexp) = compiled else {
                    return;
                };
                Needle::Regexp(regexp)
            }
        };
        let done = &mut |done| report(SearchReport(done));
        run_isearch(
            (&text, &invalid),
            &needle,
            (find, current),
            &stop,
            generation,
            done,
        );
    }
}

/// Compiles an isearch's `pattern` too large or heavy for a key, and builds the regexps for a
/// cut with it, as the worker has time for them: `None` if `stop` said so first, or the
/// pattern's error.
fn compile_on_worker(
    pattern: &str,
    insensitive: bool,
    stop: &dyn Fn() -> bool,
) -> Option<Result<Regexp, String>> {
    let compiled =
        regexp::compile_isearch(pattern, insensitive, regexp::On::Worker).map_err(|error| {
            match error {
                regexp::CompileError::Invalid(why) => why,
                // Neither on the worker, once the UI thread found it complete.
                regexp::CompileError::Incomplete | regexp::CompileError::ForWorker => {
                    "Invalid regexp".to_owned()
                }
            }
        });
    if stop() {
        return None;
    }
    if let Ok(regexp) = &compiled {
        regexp.build_cut();
    }
    (!stop()).then_some(compiled)
}

/// Does an isearch job's work once its needle is compiled: the search to go on with, then the
/// count, each reported as it ends, unless `stop` says so first. A line too long for a
/// regexp ends it with a report of its own.
fn run_isearch(
    snapshot: (&Rope, &InvalidBytes),
    needle: &Needle,
    (find, mut current): (Option<Find>, Option<usize>),
    stop: &dyn Fn() -> bool,
    generation: u64,
    report: &mut dyn FnMut(Report),
) {
    if let Some(find) = find {
        let found = match find {
            Find::Forward { from, after_match } => needle.find_forward(
                snapshot,
                (from, after_match),
                (Limits::NONE, stop),
                &mut None,
            ),
            Find::Backward {
                before,
                start_at_most,
            } => needle.find_backward(
                snapshot,
                (before, start_at_most),
                (Limits::NONE, stop),
                &mut None,
            ),
        };
        let at = match found {
            Ok(Found::Match(range)) => Some(range),
            Ok(Found::None) => None,
            Ok(Found::Stopped { .. }) => return,
            Err(LineTooLong(at)) => {
                report(Report::LineTooLong { generation, at });
                return;
            }
        };
        current = at.as_ref().map(|range| range.start);
        report(Report::Found { generation, at });
    }
    match needle.count(snapshot, current, stop) {
        Ok(Some(count)) => report(Report::Counted {
            generation,
            count,
            at: current,
        }),
        Ok(None) => {}
        Err(LineTooLong(at)) => report(Report::LineTooLong { generation, at }),
    }
}
