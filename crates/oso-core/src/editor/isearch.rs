//! isearch on the editor (ADR 0082): starting a search in the selected window, its keys,
//! where each search goes from, and how it ends. What it holds is in [`state`], its worker in
//! [`worker`] and the cells of its matches in [`cells`].
//!
//! Must not depend on: `command` or `keymap`, or the host's window or threads.
//!
//! - Only the primary cursor searches and moves. Each key searches within [`KEY_BUDGET`]
//!   here, and a worker goes on with the rest and counts the matches over a snapshot
//!   ([`SearchJob`](crate::search::SearchJob)), which a newer job supersedes through a
//!   generation.
//! - A search keeps to the window and the buffer it began in: it ends when the window shows
//!   another buffer or a prompt opens over it, so nothing it does reaches another buffer.
//! - It remembers the buffer's revision, and searches again from where it is when the text
//!   has changed under it.
//! - Nothing here logs the string, and `Debug` prints none of it (ADR 0044).

use std::ops::Range;

use super::Editor;
use crate::kill::KillEntry;
use crate::motion::{self, WalkBudget};
use crate::search::{self, Find, Found, KEY_BUDGET};
use crate::word;
use state::{Asked, Step};

mod cells;
mod state;
mod worker;

pub use cells::MatchCells;
pub use state::Isearch;
pub(super) use state::Tally;

/// The name of the search history among the prompts' (ADR 0072).
const HISTORY: &str = "isearch";

/// The most bytes a character of the string can take in the text, for what `C-w` reads.
const MAX_CHAR_BYTES: usize = 4;

/// The newest kill's text, its pieces joined by line breaks, cut to `chars` characters, so
/// that only what the string can hold is copied, however large the kill.
fn kill_text(entry: &KillEntry, chars: usize) -> String {
    let mut out = String::new();
    let mut left = chars;
    for (index, piece) in entry.pieces().iter().enumerate() {
        if index > 0 {
            if left == 0 {
                break;
            }
            out.push('\n');
            left -= 1;
        }
        for c in piece.chunks().flat_map(str::chars) {
            if left == 0 {
                return out;
            }
            out.push(c);
            left -= 1;
        }
    }
    out
}

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
        let begin = (window.selections.clone(), window.scroll());
        let search = Isearch::new(
            window.buffer(),
            begin,
            buffer.revision(),
            backward,
            around_secret,
        );
        self.isearch = Some(search);
        self.supersede_search();
        true
    }

    /// `C-s` or `C-r` in isearch: the next match that way, round the end after a failure in
    /// the same direction, or with no string, the last string searched for.
    pub fn isearch_repeat(&mut self, backward: bool) {
        self.isearch_follow_text();
        let cursor = self.primary_cursor();
        let len = self.current().1.text().len();
        let Some(state) = &self.isearch else {
            return;
        };
        if state.string.is_empty() {
            let Some(last) = self.histories.get(HISTORY, 0).map(str::to_owned) else {
                return;
            };
            let Some(state) = &mut self.isearch else {
                return;
            };
            state.push_step(cursor);
            state.backward = backward;
            state.add(&last);
            state.rebuild_query();
            self.isearch_typed();
            return;
        }
        let Some(state) = &mut self.isearch else {
            return;
        };
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
        self.isearch_find(find, Asked::Repeat { wrapped: wrap });
    }

    /// Adds `text` to the string, as typing does, and searches again from the match.
    pub fn isearch_add(&mut self, text: &str) {
        self.isearch_follow_text();
        let cursor = self.primary_cursor();
        let secret = self.shows_secret();
        let Some(state) = &mut self.isearch else {
            return;
        };
        let len = state.string.len();
        state.push_step(cursor);
        if !state.add(text) {
            state.steps.pop_back();
            return;
        }
        debug_assert!(state.string.len() > len);
        state.around_secret |= secret;
        state.rebuild_query();
        self.isearch_typed();
    }

    /// `C-w`: adds the word after the match, or the character there if no word starts there.
    pub fn isearch_yank_word(&mut self) {
        let Some(state) = &self.isearch else {
            return;
        };
        let from = state
            .current
            .as_ref()
            .filter(|_| !state.string.is_empty())
            .map_or_else(|| self.primary_cursor(), |range| range.end);
        let most = state.room().saturating_mul(MAX_CHAR_BYTES);
        let buffer = self.current().1;
        let display = buffer.display();
        let end = if word::word_at(&display, from) {
            // No further than the string can hold.
            word::forward_word(&display, from, &mut WalkBudget::with_limit(most))
        } else {
            motion::next_cluster(&display, from).unwrap_or(from)
        };
        let end = end.min(from.saturating_add(most));
        let piece: String = buffer.text().slice(from..end).chunks().collect();
        self.isearch_add(&piece);
    }

    /// `C-y`: adds the newest kill, its pieces joined by line breaks. A kill from a buffer
    /// that may hold secrets makes the searched buffer one, as a yank does (ADR 0074), so that
    /// where the search goes says no more of the kill than its text would.
    pub fn isearch_yank_kill(&mut self) {
        let Some(state) = &self.isearch else {
            return;
        };
        let room = state.room();
        let Some(entry) = self.kills.ring.get(0) else {
            return;
        };
        let (text, secret) = (kill_text(entry, room), entry.is_secret());
        if secret {
            let id = state.buffer;
            if let Some(state) = &mut self.isearch {
                state.around_secret = true;
            }
            if self
                .buffer_mut(id)
                .is_some_and(|buffer| buffer.treat_as_secret())
            {
                self.note_secret_shown();
                self.message("Buffer now treated as secret");
            }
        }
        self.isearch_add(&text);
    }

    /// `M-p` (`older`) and `M-n`: the string becomes an older or newer one of the history,
    /// and is searched for from the match.
    pub fn isearch_history(&mut self, older: bool) {
        self.isearch_follow_text();
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
        state.push_replacing_step(cursor);
        state.history = Some(index);
        state.string.clear();
        state.add(&entry);
        state.rebuild_query();
        self.isearch_typed();
    }

    /// `M-c`: turns smart case around for the rest of this search.
    pub fn isearch_toggle_case(&mut self) {
        self.isearch_follow_text();
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
        self.isearch_follow_text();
        let Some(step) = self
            .isearch
            .as_mut()
            .and_then(|state| state.steps.pop_back())
        else {
            return false;
        };
        self.isearch_restore(step);
        true
    }

    /// Ends the search where it is, as `RET` or a key isearch does not bind does: puts the
    /// string in the history, and, if the cursor moved and no region shows, the place it
    /// began on the primary cursor's mark ring. Returns whether one was under way.
    pub fn isearch_exit(&mut self) -> bool {
        let Some(state) = self.isearch.take() else {
            return false;
        };
        self.supersede_search();
        let secret = state.around_secret || self.shows_secret();
        if !secret {
            self.histories.record(HISTORY, &state.string);
        }
        let began = state.begin.0.primary().cursor;
        let selections = &mut self.windows[self.selected].selections;
        if !selections.mark_active().is_active() && selections.primary().cursor != began {
            selections.push_primary_mark(began);
            // That it moved says the search matched: around a secret, the screen alone.
            let saved = "Mark saved where search started";
            if secret {
                self.echo(saved);
            } else {
                self.message(saved);
            }
        }
        true
    }

    /// Ends a search with nothing kept, when its window shows another buffer or a prompt
    /// opens over it: no history, no mark, and the cursors stay.
    pub(super) fn isearch_abandon(&mut self) {
        if self.isearch.take().is_some() {
            self.supersede_search();
        }
    }

    /// `C-g` in isearch, ADR 0072's step 5: with a failing part or a search still running,
    /// goes back to the last string that matched; otherwise ends the search and puts the
    /// cursors and the view back as they were. `None` with no isearch.
    pub(super) fn isearch_quit(&mut self) -> Option<super::Quit> {
        self.isearch_follow_text();
        let state = self.isearch.as_mut()?;
        if state.failing || state.searching.is_some() {
            // The newest step that did not fail, restored once.
            let mut last = None;
            while let Some(step) = state.steps.pop_back() {
                let failing = step.failing;
                last = Some(step);
                if !failing {
                    break;
                }
            }
            match last {
                Some(step) => self.isearch_restore(step),
                None => {
                    // What remains is the place it began.
                    state.failing = false;
                    state.searching = None;
                    self.supersede_search();
                }
            }
            if let Some(state) = &mut self.isearch {
                state.failing = false;
            }
            return Some(super::Quit::SearchStep);
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
        self.isearch_find(find, Asked::Typed);
    }

    /// Searches from `find` within a key's budget, and leaves the rest to a worker.
    fn isearch_find(&mut self, find: Find, asked: Asked) {
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
        if let Some(state) = &mut self.isearch {
            // This search takes the place of any a worker went on with.
            state.searching = None;
        }
        match found {
            Found::Match(range) => self.isearch_matched(range, asked, false),
            Found::None => {
                if let Some(state) = &mut self.isearch {
                    state.failing = true;
                }
                if asked == Asked::Typed {
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
                    state.searching = Some((asked, find));
                    state.tally = Some(Tally::Counting { moves: 0 });
                }
                self.isearch_count(Some(resumed));
            }
        }
    }

    /// Goes to `range`, found as `asked`, and keeps the count up to date: a repeat steps it,
    /// a wrap goes to the first or the last, and anything else counts again. `by_worker`
    /// says the worker that found it is counting for it already.
    fn isearch_matched(&mut self, range: Range<usize>, asked: Asked, by_worker: bool) {
        let Some(state) = &mut self.isearch else {
            return;
        };
        let backward = state.backward;
        state.current = Some(range.clone());
        state.failing = false;
        let mut recount = false;
        if !by_worker {
            match (asked, state.tally) {
                (Asked::Repeat { wrapped }, Some(Tally::Known { index, total })) if index > 0 => {
                    let index = match (wrapped, backward) {
                        (true, false) => 1,
                        (true, true) => total,
                        (false, false) => (index + 1).min(total),
                        (false, true) => index.saturating_sub(1).max(1),
                    };
                    state.tally = Some(Tally::Known { index, total });
                }
                (Asked::Repeat { wrapped: false }, Some(Tally::Counting { moves })) => {
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
        match step.replaced {
            Some(string) => state.string = string,
            None => state.string.truncate(step.len),
        }
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
