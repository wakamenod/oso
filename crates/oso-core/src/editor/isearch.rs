//! isearch on the editor (ADR 0082): starting a search in the selected window, its keys,
//! where each search goes from, and how it ends. What it holds is in [`state`], and its
//! worker and the cells of its matches in [`worker`].
//!
//! Must not depend on: `command` or `keymap`, or the host's window or threads.
//!
//! - Only the primary cursor searches and moves. Each key searches within
//!   [`KEY_BUDGET`] here, and a worker goes on with the rest and counts the matches over a
//!   snapshot ([`SearchJob`](search::SearchJob)), which a newer job supersedes through a generation.
//! - isearch remembers the buffer's revision, and searches again from where it is when the
//!   text has changed under it, as a load still under way changes it.
//! - Nothing here logs the string, and `Debug` prints none of it (ADR 0044).

use std::ops::Range;

use super::Editor;
use crate::motion::{self, WalkBudget};
use crate::search::{self, Find, Found, KEY_BUDGET, MAX_QUERY_CHARS};
use crate::word;
use state::{How, MAX_STEPS, Step};

mod state;
mod worker;

pub use state::{Isearch, Tally};

/// The name of the search history among the prompts' (ADR 0072).
pub(super) const HISTORY: &str = "isearch";

impl Editor {
    /// The isearch under way, if any.
    pub fn isearch(&self) -> Option<&Isearch> {
        self.isearch.as_ref()
    }

    /// Starts an isearch in the selected window, forward or `backward`. Returns false in a
    /// prompt, where it does not work.
    pub fn isearch_begin(&mut self, backward: bool) -> bool {
        if !self.prompts.is_empty() {
            return false;
        }
        let around_secret = self.shows_secret();
        let (window, buffer) = self.current();
        self.isearch = Some(Isearch {
            string: String::new(),
            backward,
            flipped: false,
            query: None,
            current: None,
            failing: false,
            wrapped: false,
            searching: None,
            tally: None,
            buffer: window.buffer(),
            begin: (window.selections.clone(), window.scroll()),
            revision: buffer.revision(),
            steps: Vec::new(),
            around_secret,
            history: None,
        });
        self.supersede_search();
        true
    }

    /// `C-s` or `C-r` in isearch: the next match that way, round the end after a failure in
    /// the same direction, or with no string, the last string searched for.
    pub fn isearch_repeat(&mut self, backward: bool) {
        self.isearch_fresh();
        let cursor = self.primary_cursor();
        let last = self.histories.get(HISTORY, 0).map(str::to_owned);
        let len = self.current().1.text().len();
        let Some(state) = &mut self.isearch else {
            return;
        };
        if state.string.is_empty() {
            if let Some(last) = last {
                state.push_step(cursor);
                state.backward = backward;
                state.add(&last);
                state.rebuild_query();
                self.isearch_typed();
            }
            return;
        }
        if state.searching.is_some() {
            return;
        }
        state.push_step(cursor);
        let turned = state.backward != backward;
        state.backward = backward;
        let wrap = state.failing && !turned;
        let here = state.current.clone();
        let find = if backward {
            let before = if wrap {
                len
            } else {
                here.map_or(cursor, |range| range.start)
            };
            Find::Backward {
                before,
                start_at_most: before,
            }
        } else {
            let from = if wrap {
                0
            } else {
                here.map_or(cursor, |range| range.end)
            };
            Find::Forward { from }
        };
        if wrap {
            state.wrapped = true;
        }
        self.isearch_find(find, How::Repeat { wrapped: wrap });
    }

    /// Adds `text` to the string, as typing does, and searches again from the match.
    pub fn isearch_add(&mut self, text: &str) {
        self.isearch_fresh();
        let cursor = self.primary_cursor();
        let secret = self.shows_secret();
        let Some(state) = &mut self.isearch else {
            return;
        };
        let before = state.step(cursor);
        if !state.add(text) {
            return;
        }
        state.around_secret |= secret;
        if state.steps.len() == MAX_STEPS {
            state.steps.remove(0);
        }
        state.steps.push(before);
        state.rebuild_query();
        self.isearch_typed();
    }

    /// `C-w`: adds the word after the match, or the character there if it starts none.
    pub fn isearch_yank_word(&mut self) {
        let Some(state) = &self.isearch else {
            return;
        };
        let from = state
            .current
            .as_ref()
            .filter(|_| !state.string.is_empty())
            .map_or_else(|| self.primary_cursor(), |range| range.end);
        let buffer = self.current().1;
        let display = buffer.display();
        let text = buffer.text();
        let starts_word = text
            .chunks_in_range(from..text.len())
            .flat_map(str::chars)
            .next()
            .is_some_and(char::is_alphanumeric);
        let end = if starts_word {
            word::forward_word(&display, from, &mut WalkBudget::new())
        } else {
            motion::next_cluster(&display, from).unwrap_or(from)
        };
        // A word longer than the string can hold is cut in `add`, after this copy.
        let end = end.min(from.saturating_add(MAX_QUERY_CHARS * 4));
        let piece: String = text.slice(from..end).chunks().collect();
        self.isearch_add(&piece);
    }

    /// `C-y`: adds the newest kill, its pieces joined by line breaks. A kill from a buffer
    /// that may hold secrets puts the search around one.
    pub fn isearch_yank_kill(&mut self) {
        let Some(entry) = self.kills.ring.get(0) else {
            return;
        };
        let (text, secret) = (entry.joined(), entry.is_secret());
        if let Some(state) = &mut self.isearch {
            state.around_secret |= secret;
        }
        self.isearch_add(&text);
    }

    /// `M-p` (`older`) and `M-n`: the string becomes an older or newer one of the history,
    /// and is searched for from the match.
    pub fn isearch_history(&mut self, older: bool) {
        self.isearch_fresh();
        let cursor = self.primary_cursor();
        let Some(state) = &self.isearch else {
            return;
        };
        let index = match (state.history, older) {
            (None, true) => Some(0),
            (None, false) => None,
            (Some(at), true) => Some(at + 1),
            (Some(at), false) => at.checked_sub(1),
        };
        let Some((index, entry)) =
            index.and_then(|index| Some((index, self.histories.get(HISTORY, index)?.to_owned())))
        else {
            return;
        };
        let Some(state) = &mut self.isearch else {
            return;
        };
        state.push_step(cursor);
        state.history = Some(index);
        state.string.clear();
        state.add(&entry);
        state.rebuild_query();
        self.isearch_typed();
    }

    /// `M-c`: turns smart case around for the rest of this search.
    pub fn isearch_toggle_case(&mut self) {
        self.isearch_fresh();
        let cursor = self.primary_cursor();
        let Some(state) = &mut self.isearch else {
            return;
        };
        state.push_step(cursor);
        state.flipped = !state.flipped;
        state.rebuild_query();
        self.isearch_typed();
    }

    /// `DEL`: goes back one step, the string and the place together. Returns false with no
    /// step to go back to.
    pub fn isearch_delete(&mut self) -> bool {
        self.isearch_fresh();
        let Some(step) = self.isearch.as_mut().and_then(|state| state.steps.pop()) else {
            return false;
        };
        self.isearch_restore(step);
        true
    }

    /// Ends the search where it is, as `RET` or a key isearch does not bind does: puts the
    /// string in the history, and the place it began on the primary cursor's mark ring
    /// unless a region shows. Returns whether one was under way.
    pub fn isearch_exit(&mut self) -> bool {
        let Some(state) = self.isearch.take() else {
            return false;
        };
        self.supersede_search();
        if !state.around_secret && !self.shows_secret() {
            self.histories.record(HISTORY, &state.string);
        }
        let began = state.begin.0.primary().cursor;
        let selections = &mut self.windows[self.selected].selections;
        if !selections.mark_active().is_active() && selections.primary().cursor != began {
            selections.push_primary_mark(began);
            self.message("Mark saved where search started");
        }
        true
    }

    /// `C-g` in isearch, ADR 0072's step 5: with a failing part or a search still running,
    /// goes back to the last string that matched; otherwise ends the search and puts the
    /// cursors and the view back as they were. `None` with no isearch.
    pub(super) fn isearch_quit(&mut self) -> Option<super::Quit> {
        let state = self.isearch.as_mut()?;
        if state.failing || state.searching.is_some() {
            while let Some(step) = self.isearch.as_mut().and_then(|state| state.steps.pop()) {
                let failing = step.failing;
                self.isearch_restore(step);
                if !failing {
                    break;
                }
            }
            if let Some(state) = &mut self.isearch {
                // With no step left, what remains is the place it began.
                state.failing = false;
            }
            return Some(super::Quit::SearchFailure);
        }
        let state = self.isearch.take()?;
        self.supersede_search();
        let (mut selections, scroll) = state.begin;
        let index = self.tiled_index();
        let (window, buffer) = self.window_mut(index);
        if window.buffer() == state.buffer {
            let display = buffer.display();
            super::follow::clip_selections(&mut selections, &display, &[], false);
            window.restore_positions(selections, scroll, &display);
            self.follow_window(index);
        }
        Some(super::Quit::Search)
    }

    /// Searches for a changed string from the current match, which stays if it still
    /// matches: forward from its start, backward from as far after it as the string
    /// reaches.
    fn isearch_typed(&mut self) {
        let cursor = self.primary_cursor();
        let len = self.current().1.text().len();
        let Some(state) = &mut self.isearch else {
            return;
        };
        state.history = state.history.filter(|_| state.query.is_some());
        let Some(query) = &state.query else {
            // An empty string: back where it began.
            state.current = None;
            state.failing = false;
            state.searching = None;
            state.tally = None;
            self.supersede_search();
            return;
        };
        let start = state.current.as_ref().map_or(cursor, |range| range.start);
        let find = if state.backward {
            let reach = state.current.as_ref().map_or(cursor, |range| {
                range.start.saturating_add(query.max_bytes()).min(len)
            });
            Find::Backward {
                before: reach,
                start_at_most: start,
            }
        } else {
            Find::Forward { from: start }
        };
        self.isearch_find(find, How::Typed);
    }

    /// Searches from `find` within a key's budget, and leaves the rest to a worker.
    fn isearch_find(&mut self, find: Find, how: How) {
        let Some(state) = &self.isearch else {
            return;
        };
        let Some(query) = state.query.clone() else {
            return;
        };
        let buffer = self.current().1;
        let (text, invalid) = (buffer.text(), buffer.invalid_bytes());
        let never = || false;
        let found = match find {
            Find::Forward { from } => {
                search::find_forward(text, invalid, &query, from, KEY_BUDGET, &never)
            }
            Find::Backward {
                before,
                start_at_most,
            } => search::find_backward(
                text,
                invalid,
                &query,
                before,
                start_at_most,
                KEY_BUDGET,
                &never,
            ),
        };
        match found {
            Found::Match(range) => self.isearch_matched(range, how, false),
            Found::None => {
                if let Some(state) = &mut self.isearch {
                    state.failing = true;
                }
                if how == How::Typed {
                    self.isearch_count(None);
                }
            }
            Found::Stopped { resume } => {
                let resumed = match find {
                    Find::Forward { .. } => Find::Forward { from: resume },
                    Find::Backward { start_at_most, .. } => Find::Backward {
                        before: resume,
                        start_at_most,
                    },
                };
                if let Some(state) = &mut self.isearch {
                    state.searching = Some(how);
                    state.tally = Some(Tally::Counting { moves: 0 });
                }
                self.isearch_count(Some(resumed));
            }
        }
    }

    /// Goes to `range`, found as `how` asked, and keeps the count up to date: a repeat
    /// steps it, and anything else counts again. `by_worker` says the worker that found it
    /// is counting for it already.
    fn isearch_matched(&mut self, range: Range<usize>, how: How, by_worker: bool) {
        let Some(state) = &mut self.isearch else {
            return;
        };
        let backward = state.backward;
        state.current = Some(range.clone());
        state.failing = false;
        let mut recount = false;
        if !by_worker {
            match (how, state.tally) {
                (How::Repeat { wrapped }, Some(Tally::Known { index, total })) if index > 0 => {
                    let index = match (wrapped, backward) {
                        (true, false) => 1,
                        (true, true) => total,
                        (false, false) => (index + 1).min(total),
                        (false, true) => index.saturating_sub(1).max(1),
                    };
                    state.tally = Some(Tally::Known { index, total });
                }
                (How::Repeat { wrapped: false }, Some(Tally::Counting { moves })) => {
                    let step = if backward { -1 } else { 1 };
                    state.tally = Some(Tally::Counting {
                        moves: moves + step,
                    });
                }
                _ => recount = true,
            }
        }
        let offset = if backward { range.start } else { range.end };
        let buffer = self.current().1;
        let cursor = motion::cluster_start(&buffer.display(), offset);
        self.windows[self.selected].selections.move_primary(cursor);
        if recount {
            self.isearch_count(None);
        }
        if by_worker {
            self.follow_cursor();
        }
    }

    /// Goes back to `step`, and counts again unless the count it had still holds.
    fn isearch_restore(&mut self, step: Step) {
        self.supersede_search();
        let Some(state) = &mut self.isearch else {
            return;
        };
        state.string.truncate(step.len);
        state.backward = step.backward;
        state.flipped = step.flipped;
        state.current = step.current;
        state.failing = step.failing;
        state.wrapped = step.wrapped;
        state.searching = None;
        state.tally = step.tally;
        state.rebuild_query();
        let recount = state.query.is_some() && !matches!(state.tally, Some(Tally::Known { .. }));
        let buffer = self.current().1;
        let cursor = motion::cluster_start(&buffer.display(), step.cursor);
        self.windows[self.selected].selections.move_primary(cursor);
        if recount {
            self.isearch_count(None);
        }
    }

    /// The selected window's primary cursor.
    fn primary_cursor(&self) -> usize {
        self.window().selections.primary().cursor
    }

    /// The echo line of an isearch under way, after what `out` holds.
    pub(super) fn write_isearch_line(&self, out: &mut String) {
        if let Some(state) = &self.isearch {
            state.write_line(out);
        }
    }
}
