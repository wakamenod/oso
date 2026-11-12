//! Where an isearch key searches (ADR 0082, ADR 0116): from the current match as the string
//! changes, or on from it for a repeat, within a key's budget and time on the UI thread, the
//! rest left to the worker, and a regexp's failure at a line too long to match in.
//!
//! Must not depend on: `command` or `keymap`, or the host's window or threads.

use std::ops::Range;
use std::time::Instant;

use super::Tally;
use super::state::{Asked, Pattern};
use crate::editor::Editor;
use crate::search::{self, Find, Found, Limits, LineTooLong, Needle};
use crate::sum_tree::Bias;

impl Editor {
    /// Where a repeat that way searches from: on from the current match, round from the
    /// buffer's end if it `wrap`s, or from the cursor with none. Past an empty match it goes a
    /// character on first, as Emacs's isearch does; `None` back from one at the start.
    pub(super) fn isearch_repeat_find(&self, backward: bool, wrap: bool) -> Option<Find> {
        let cursor = self.primary_cursor();
        let text = self.current().1.text();
        let here = self.isearch.as_ref()?.current.as_ref();
        let back_from = |range: &Range<usize>| match range.start.checked_sub(1) {
            _ if !range.is_empty() => Some(range.start),
            Some(before) => Some(text.clip_offset(before, Bias::Left)),
            None => None,
        };
        if backward {
            let before = if wrap {
                Some(text.len())
            } else {
                here.map_or(Some(cursor), back_from)
            };
            return before.map(|before| Find::Backward {
                before,
                start_at_most: before,
            });
        }
        let (from, after_match) = match here {
            _ if wrap => (0, false),
            None => (cursor, false),
            Some(range) => (search::after(text, range), !range.is_empty()),
        };
        Some(Find::Forward { from, after_match })
    }

    /// Searches for a changed string from the current match, which stays if it still
    /// matches: forward from its start, backward from as far after it as the string
    /// reaches, in the time a key has from `started`.
    pub(super) fn isearch_typed(&mut self, started: Instant) {
        let cursor = self.primary_cursor();
        let len = self.current().1.text().len();
        let Some(state) = &mut self.isearch else {
            return;
        };
        // Where `M-p` is in the history holds while there is a string, also one that does
        // not compile.
        state.history = state.history.filter(|_| !state.string.is_empty());
        if !state.has_needle() {
            state.searching = None;
            state.tally = None;
            if !matches!(state.pattern, Pattern::Bad(_)) {
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
                .map_or(cursor, |range| match state.needle() {
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
        self.isearch_find(find, Asked::Typed, started);
    }

    /// Searches from `find` within a key's budget and its time from `started`, when the key began
    /// to build what it searches for, and leaves the rest to a worker, also the compile of a
    /// regexp too large for a key.
    pub(super) fn isearch_find(&mut self, find: Find, asked: Asked, started: Instant) {
        let Some(state) = &self.isearch else {
            return;
        };
        let found = if matches!(state.pattern, Pattern::Compiling) {
            Ok(Found::Stopped {
                resume: match find {
                    Find::Forward { from, .. } => from,
                    Find::Backward { before, .. } => before,
                },
            })
        } else {
            let Some(needle) = state.needle() else {
                return;
            };
            let buffer = self.current().1;
            let snapshot = (buffer.text(), buffer.invalid_bytes());
            let never: &dyn Fn() -> bool = &|| false;
            let limits = (Limits::key(started), never);
            match find {
                Find::Forward { from, after_match } => {
                    needle.find_forward(snapshot, (from, after_match), limits, &mut None)
                }
                Find::Backward {
                    before,
                    start_at_most,
                } => needle.find_backward(snapshot, (before, start_at_most), limits, &mut None),
            }
        };
        if let Some(state) = &mut self.isearch {
            // This search takes the place of any a worker went on with.
            state.searching = None;
            state.too_long = None;
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
            Err(LineTooLong(at)) => self.isearch_line_too_long(at, Some((asked, find))),
        }
    }

    /// At a line too long to match a regexp in, which starts at `at` (ADR 0116): a search for
    /// migemo's pattern goes on with the literal string, as if `M-m` had turned migemo off, from
    /// where `again` searched, or from the match with none, and counts anew. Any other fails
    /// there and says so on its line, with no count.
    pub(super) fn isearch_line_too_long(&mut self, at: usize, again: Option<(Asked, Find)>) {
        self.supersede_search();
        if let Some(state) = &mut self.isearch
            && matches!(state.pattern, Pattern::Migemo(_))
        {
            state.use_migemo = false;
            state.searching = None;
            let started = self.isearch_rebuild();
            // The pattern changed, so the count starts again, as for a typed string.
            match again {
                Some((_, find)) => self.isearch_find(find, Asked::Typed, started),
                None => self.isearch_typed(started),
            }
            return;
        }
        let row = self.current().1.text().offset_to_point(at).row + 1;
        if let Some(state) = &mut self.isearch {
            state.failing = true;
            state.searching = None;
            state.tally = None;
            state.too_long = Some(row);
        }
    }
}

#[cfg(test)]
impl Editor {
    /// Matches the search's regexp in lines at most `longest` bytes long, for a test of the
    /// rule, until the string changes.
    pub(in crate::editor) fn isearch_longest_line(&mut self, longest: usize) {
        if let Some(state) = &mut self.isearch
            && let Pattern::Ready(Needle::Regexp(regexp)) | Pattern::Migemo(Needle::Regexp(regexp)) =
                &mut state.pattern
        {
            regexp.set_longest_line(longest);
        }
    }
}
