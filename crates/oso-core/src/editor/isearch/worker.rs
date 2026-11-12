//! isearch's worker (ADR 0082): the job a search leaves for a thread, what comes back from
//! it, a regexp it compiled among that (ADR 0116), and a search again when the text changed
//! under it.
//!
//! Must not depend on: `command` or `keymap`, or the host's window or threads. The host
//! runs the job; this only makes it and takes in what it reports.
//!
//! A newer job supersedes the one before through a generation, which the worker checks
//! every block of text, and before every look for a regexp's match, and whose reports are
//! dropped here once it is not the live one.
//! A report is also dropped once the search no longer keeps to the window it began in.

use std::ops::Range;
use std::sync::atomic::Ordering;

use super::Tally;
use super::state::{Asked, Bad, Pattern};
use crate::editor::Editor;
use crate::search::{Find, Needle, Regexp, Report, SearchJob, SearchReport};

impl Editor {
    /// The job a worker is to do, once, for the host.
    pub fn take_isearch_job(&mut self) -> Option<SearchJob> {
        self.search_job.take()
    }

    /// Takes in what a worker found, unless a newer job has taken its place. Returns whether
    /// what shows changed.
    pub fn isearch_reported(&mut self, report: SearchReport) -> bool {
        // query-replace shares the worker, and is never on with isearch (ADR 0113).
        if self.replace().is_some() {
            return self.replace_reported(report);
        }
        let report = match report.0 {
            Report::Matches {
                generation,
                matched,
            } => return self.cursors_reported(generation, matched),
            report => report,
        };
        let live = self.search_live.load(Ordering::Relaxed);
        if self.isearch_follow_text() {
            return true;
        }
        let Some(state) = &mut self.isearch else {
            return false;
        };
        match report {
            Report::Found { generation, at } if generation == live => {
                let Some((asked, _)) = state.searching.take() else {
                    return false;
                };
                match at {
                    Some(range) => self.isearch_matched(range, asked, true),
                    None => state.failing = true,
                }
                true
            }
            Report::Compiled { generation, regexp } if generation == live => {
                self.isearch_compiled(regexp)
            }
            // The search's, which fails there, or the count's, which then shows nothing.
            Report::LineTooLong { generation, at } if generation == live => {
                if state.searching.is_some() {
                    self.isearch_line_too_long(at);
                } else {
                    state.tally = None;
                }
                true
            }
            Report::Counted {
                generation,
                count,
                at,
            } if generation == live => {
                let Some(Tally::Counting { moves }) = state.tally else {
                    return false;
                };
                let index = match at {
                    Some(_) if count.total > 0 => {
                        let index = (count.before + 1).saturating_add_signed(moves);
                        index.clamp(1, count.total)
                    }
                    _ => 0,
                };
                state.tally = Some(Tally::Known {
                    index,
                    total: count.total,
                });
                true
            }
            _ => false,
        }
    }

    /// Takes in the regexp a worker compiled for the search, or why it could not; the search
    /// and count of the same job follow it. Returns whether what shows changed.
    fn isearch_compiled(&mut self, regexp: Result<Regexp, String>) -> bool {
        let Some(state) = &mut self.isearch else {
            return false;
        };
        if !matches!(state.pattern, Pattern::Compiling) {
            return false;
        }
        state.pattern = match regexp {
            Ok(regexp) => Pattern::Ready(Needle::Regexp(regexp)),
            Err(why) => {
                state.searching = None;
                state.tally = None;
                Pattern::Bad(Bad::Invalid(why))
            }
        };
        true
    }

    /// Keeps the search true to its window and its text: ends it if the selected window no
    /// longer shows its buffer, or a prompt is open, and searches again if the text changed
    /// since it last looked. While the buffer still loads, only the count waits for the
    /// load to end, so a load's updates do not restart the search each time. Returns
    /// whether what shows changed. The host calls this before it waits.
    pub fn isearch_follow_text(&mut self) -> bool {
        let Some(state) = &self.isearch else {
            return false;
        };
        let shown = self.prompts.is_empty() && self.window().buffer() == state.buffer;
        let Some(buffer) = self.buffer(state.buffer).filter(|_| shown) else {
            self.isearch_abandon();
            return true;
        };
        let (revision, len, loading) = (
            buffer.revision(),
            buffer.text().len(),
            buffer.loading().is_some(),
        );
        let Some(state) = &mut self.isearch else {
            return false;
        };
        if revision == state.revision {
            return false;
        }
        if loading {
            // A count of a part would be wrong, and one per update never ends.
            let counting = matches!(state.tally, Some(Tally::Counting { .. }));
            if state.has_needle() && !counting {
                state.tally = Some(Tally::Counting { moves: 0 });
                self.supersede_search();
                return true;
            }
            return false;
        }
        state.revision = revision;
        let pending = state.searching.take();
        let clip = |range: Option<Range<usize>>| {
            range
                .map(|range| range.start.min(len)..range.end.min(len))
                .filter(|range| !range.is_empty())
        };
        state.current = clip(state.current.take());
        for step in &mut state.steps {
            step.current = clip(step.current.take());
            step.cursor = step.cursor.min(len);
            step.tally = None;
        }
        match pending {
            // A repeat a worker went on with is asked again, over the new text.
            Some((asked @ Asked::Repeat { .. }, find)) => {
                let find = match find {
                    Find::Forward { from, after_match } => Find::Forward {
                        from: from.min(len),
                        after_match,
                    },
                    Find::Backward {
                        before,
                        start_at_most,
                    } => Find::Backward {
                        before: before.min(len),
                        start_at_most: start_at_most.min(len),
                    },
                };
                self.isearch_find(find, asked);
            }
            _ => self.isearch_typed(),
        }
        true
    }

    /// Asks a worker to go on with `find`, if any, and then to count the matches, for the
    /// current match unless it fails. Supersedes any job before.
    pub(super) fn isearch_count(&mut self, find: Option<Find>) {
        let generation = self.supersede_search();
        let Some(state) = &mut self.isearch else {
            return;
        };
        let Some(needle) = state.prepared() else {
            return;
        };
        if find.is_none() {
            state.tally = Some(Tally::Counting { moves: 0 });
        }
        let current = state
            .current
            .as_ref()
            .filter(|_| !state.failing)
            .map(|range| range.start);
        let id = state.buffer;
        let Some(buffer) = self.buffer(id) else {
            return;
        };
        let snapshot = (buffer.text().clone(), buffer.invalid_bytes().clone());
        let live = (generation, self.search_live.clone());
        self.search_job = Some(SearchJob::new(snapshot, needle, find, current, live));
    }

    /// Ends whatever a worker is doing, and returns the generation of the next job.
    pub(in crate::editor) fn supersede_search(&mut self) -> u64 {
        self.search_job = None;
        let generation = self.search_live.load(Ordering::Relaxed).wrapping_add(1);
        self.search_live.store(generation, Ordering::Relaxed);
        generation
    }
}
