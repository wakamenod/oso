//! Where an isearch key searches (ADR 0082, ADR 0116): from the current match as the string
//! changes, within a key's budget and time on the UI thread, the rest left to the worker, and
//! a regexp's failure at a line too long to match in.
//!
//! Must not depend on: `command` or `keymap`, or the host's window or threads.

use super::Tally;
use super::state::{Asked, Problem};
use crate::editor::Editor;
use crate::search::{Find, Found, Limits, LineTooLong, Needle};

impl Editor {
    /// Searches for a changed string from the current match, which stays if it still
    /// matches: forward from its start, backward from as far after it as the string
    /// reaches.
    pub(super) fn isearch_typed(&mut self) {
        let cursor = self.primary_cursor();
        let len = self.current().1.text().len();
        let Some(state) = &mut self.isearch else {
            return;
        };
        state.history = state.history.filter(|_| state.has_needle());
        if !state.has_needle() {
            state.searching = None;
            state.tally = None;
            if state.problem.is_none() {
                // An empty string: back where it began.
                state.current = None;
                state.failing = false;
            }
            // A regexp that does not compile stays where it was, as Emacs's does.
            self.supersede_search();
            return;
        }
        let start = state.current.as_ref().map_or(cursor, |range| range.start);
        let find = if state.backward {
            let began = state.begin.0.primary().cursor;
            let reach = state
                .current
                .as_ref()
                .map_or(cursor, |range| match &state.needle {
                    Some(Needle::Literal(query)) => {
                        range.start.saturating_add(query.max_bytes()).min(len)
                    }
                    // A regexp's match ends by where the search began, as in Emacs, or anywhere
                    // once it went round.
                    _ if state.wrapped => len,
                    _ => began.max(range.end),
                });
            Find::Backward {
                before: reach,
                start_at_most: start,
            }
        } else {
            Find::Forward {
                from: start,
                after_match: false,
            }
        };
        self.isearch_find(find, Asked::Typed);
    }

    /// Searches from `find` within a key's budget, and leaves the rest to a worker, also the
    /// compile of a regexp too large for a key.
    pub(super) fn isearch_find(&mut self, find: Find, asked: Asked) {
        let Some(state) = &self.isearch else {
            return;
        };
        let found = if state.compiling {
            Ok(Found::Stopped {
                resume: match find {
                    Find::Forward { from, .. } => from,
                    Find::Backward { before, .. } => before,
                },
            })
        } else {
            let Some(needle) = &state.needle else {
                return;
            };
            let buffer = self.current().1;
            let snapshot = (buffer.text(), buffer.invalid_bytes());
            let (never, limits) = (|| false, Limits::key());
            match find {
                Find::Forward { from, after_match } => {
                    needle.find_forward(snapshot, (from, after_match), limits, &never)
                }
                Find::Backward {
                    before,
                    start_at_most,
                } => needle.find_backward(snapshot, (before, start_at_most), limits, &never),
            }
        };
        if let Some(state) = &mut self.isearch {
            // This search takes the place of any a worker went on with.
            state.searching = None;
            state.problem = None;
        }
        match found {
            Ok(Found::Match(range)) => self.isearch_matched(range, asked, false),
            Ok(Found::None) => {
                if let Some(state) = &mut self.isearch {
                    state.failing = true;
                }
                if asked == Asked::Typed {
                    self.isearch_count(None);
                }
            }
            Ok(Found::Stopped { resume }) => {
                let resumed = match find {
                    Find::Forward { from, after_match } => Find::Forward {
                        from: resume,
                        after_match: after_match && resume == from,
                    },
                    Find::Backward { start_at_most, .. } => Find::Backward {
                        before: resume,
                        start_at_most,
                    },
                };
                if let Some(state) = &mut self.isearch {
                    state.searching = Some((asked, find));
                    state.tally = Some(Tally::Counting { moves: 0 });
                }
                self.isearch_count(Some(resumed));
            }
            Err(LineTooLong(at)) => self.isearch_line_too_long(at),
        }
    }

    /// Fails the search at a line too long to match a regexp in, which starts at `at`, and
    /// says so on its line, with no count (ADR 0116).
    pub(super) fn isearch_line_too_long(&mut self, at: usize) {
        self.supersede_search();
        let row = self.current().1.text().offset_to_point(at).row + 1;
        if let Some(state) = &mut self.isearch {
            state.failing = true;
            state.searching = None;
            state.tally = None;
            state.problem = Some(Problem::LineTooLong(row));
        }
    }
}

#[cfg(test)]
impl Editor {
    /// Matches the search's regexp in lines at most `longest` bytes long, for a test of the
    /// rule, until the string changes.
    pub(in crate::editor) fn isearch_longest_line(&mut self, longest: usize) {
        if let Some(state) = &mut self.isearch
            && let Some(Needle::Regexp(regexp)) = &mut state.needle
        {
            regexp.set_longest_line(longest);
        }
    }
}
