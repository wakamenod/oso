//! isearch on the editor (ADR 0082, ADR 0116): starting a search in the selected window,
//! literal or a regexp's, its keys, and how it ends. What it holds is in [`state`], where
//! each key searches from in [`find`], its worker in [`worker`], the cells of its matches in
//! [`cells`], and `C-y` and `M-y` in [`yank`]. Migemo's setting and needles are in
//! [`migemo`](super::migemo).
//!
//! Must not depend on: `command` or `keymap`, or the host's window or threads.
//!
//! - Only the primary cursor searches and moves. Each key searches within
//!   [`KEY_BUDGET`](search::KEY_BUDGET) and 3 ms here, and a worker goes on with the rest
//!   and counts the matches over a snapshot ([`SearchJob`](crate::search::SearchJob)), which
//!   a newer job supersedes through a generation. A regexp too large to compile within a
//!   key's limit compiles there too.
//! - A search keeps to the window and the buffer it began in: it ends when the window shows
//!   another buffer or a prompt opens over it, so nothing it does reaches another buffer.
//! - It remembers the buffer's revision, and searches again from where it is when the text
//!   has changed under it.
//! - Nothing here logs the string, and `Debug` prints none of it (ADR 0044).

use std::ops::Range;
use std::time::Instant;

use super::Editor;
use crate::kill::KillEntry;
use crate::motion::{self, WalkBudget};
use crate::search::{self, Needle};
use crate::word;
use state::{Asked, Step};

mod cells;
mod find;
mod state;
mod worker;
mod yank;

pub use cells::MatchCells;
pub use state::Isearch;
pub(super) use state::Tally;

/// The name of the search history among the prompts' (ADR 0072), and of the regexp search's,
/// which is kept apart, as Emacs keeps `regexp-search-ring` (ADR 0116).
fn history_name(regexp: bool) -> &'static str {
    if regexp { "isearch-regexp" } else { "isearch" }
}

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

    /// Starts an isearch in the selected window, forward or `backward`, for a `regexp` or a
    /// literal string. Returns false in a prompt, where it does not work.
    pub fn isearch_begin(&mut self, backward: bool, regexp: bool) -> bool {
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
            (backward, regexp, self.migemo_on()),
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
        let Some(state) = &self.isearch else {
            return;
        };
        if state.string.is_empty() {
            let name = history_name(state.regexp);
            let Some(last) = self.histories.get(name, 0).map(str::to_owned) else {
                return;
            };
            let Some(state) = &mut self.isearch else {
                return;
            };
            state.push_step(cursor);
            state.backward = backward;
            state.add(&last);
            let started = self.isearch_rebuild();
            self.isearch_typed(started);
            return;
        }
        // A regexp that does not compile has nothing to repeat.
        if state.searching.is_some() || !state.has_needle() {
            return;
        }
        let wrap = state.failing && state.backward == backward;
        let find = self.isearch_repeat_find(backward, wrap);
        let Some(state) = &mut self.isearch else {
            return;
        };
        state.push_step(cursor);
        state.backward = backward;
        if wrap {
            state.wrapped = true;
        }
        match find {
            Some(find) => self.isearch_find(find, Asked::Repeat { wrapped: wrap }, Instant::now()),
            // An empty match at the start: none before it, as at a failure.
            None => state.failing = true,
        }
    }

    /// Adds `text` to the string, as typing does, and searches again from the match.
    pub fn isearch_add(&mut self, text: &str) {
        self.isearch_follow_text();
        let cursor = self.primary_cursor();
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
        let started = self.isearch_rebuild();
        self.isearch_typed(started);
    }

    /// Adds `text` taken from the buffer or a kill, as `C-w` and `C-y` do: as it is to a
    /// literal string, and with its special characters escaped to a regexp, as Emacs's
    /// `isearch-yank-string` quotes it.
    fn isearch_add_taken(&mut self, text: &str) {
        match &self.isearch {
            Some(state) if state.regexp => self.isearch_add(&search::quote(text, state.room())),
            _ => self.isearch_add(text),
        }
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
        self.isearch_add_taken(&piece);
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
        let name = history_name(state.regexp);
        let Some((index, entry)) =
            index.and_then(|index| Some((index, self.histories.get(name, index)?.to_owned())))
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
        let started = self.isearch_rebuild();
        self.isearch_typed(started);
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
        let started = self.isearch_rebuild();
        self.isearch_typed(started);
    }

    /// `M-r`: turns the string from literal to a regexp or back, for the rest of this search,
    /// and searches again from the match. `M-p` and `M-n` then go through the other history.
    pub fn isearch_toggle_regexp(&mut self) {
        self.isearch_follow_text();
        let cursor = self.primary_cursor();
        let Some(state) = &mut self.isearch else {
            return;
        };
        state.push_step(cursor);
        state.regexp = !state.regexp;
        state.history = None;
        let started = self.isearch_rebuild();
        self.isearch_typed(started);
    }

    /// `M-m`: turns migemo off or on for the rest of this search, and searches again from
    /// the match (ADR 0116). In a regexp's search it shows once `M-r` makes the string
    /// literal.
    pub fn isearch_toggle_migemo(&mut self) {
        self.isearch_follow_text();
        let cursor = self.primary_cursor();
        let Some(state) = &mut self.isearch else {
            return;
        };
        state.push_step(cursor);
        state.use_migemo = !state.use_migemo;
        let started = self.isearch_rebuild();
        self.isearch_typed(started);
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
            self.histories
                .record(history_name(state.regexp), &state.string);
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

    /// What the search looks for, its current match, how many matches it counted if it
    /// knows, and whether it goes backward, for the keys that add cursors from it (ADR 0091).
    /// None while it fails, its regexp does not compile, or a worker still looks for the
    /// match.
    pub(super) fn isearch_current_match(
        &self,
    ) -> Option<(Needle, Range<usize>, Option<usize>, bool)> {
        let state = self.isearch.as_ref()?;
        if state.failing || state.searching.is_some() {
            return None;
        }
        let total = match state.tally {
            Some(Tally::Known { total, .. }) => Some(total),
            _ => None,
        };
        Some((
            state.needle()?.clone(),
            state.current.clone()?,
            total,
            state.backward,
        ))
    }

    /// Ends a search with nothing kept, when its window shows another buffer or a prompt
    /// opens over it: no history, no mark, and the cursors stay.
    pub(super) fn isearch_abandon(&mut self) {
        if self.isearch.take().is_some() {
            self.supersede_search();
        }
    }

    /// `C-g` in isearch, ADR 0072's step 5: with a failing part, a regexp that does not
    /// compile, or a search still running, goes back to the last string that matched;
    /// otherwise ends the search and puts the cursors and the view back as they were. `None`
    /// with no isearch.
    pub(super) fn isearch_quit(&mut self) -> Option<super::Quit> {
        self.isearch_follow_text();
        let state = self.isearch.as_mut()?;
        let bad = matches!(state.pattern, state::Pattern::Bad(_));
        if state.failing || state.searching.is_some() || bad {
            // The newest step that did not fail, restored once.
            let mut last = None;
            while let Some(step) = state.steps.pop_back() {
                let failing = step.failing || step.invalid;
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
                    state.too_long = None;
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
            super::follow::clip_selections(&mut selections, &display, &[], false, None);
            window.restore_positions(selections, scroll, &display);
            self.follow_window(index);
        }
        Some(super::Quit::Search)
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
        state.regexp = step.regexp;
        state.use_migemo = step.use_migemo;
        state.current = step.current;
        state.failing = step.failing;
        state.too_long = step.too_long;
        state.wrapped = step.wrapped;
        state.searching = None;
        state.tally = step.tally;
        self.isearch_rebuild();
        let Some(state) = &self.isearch else {
            return;
        };
        // A regexp the worker compiles needs its job, whatever count the step kept.
        let recount = match state.pattern {
            state::Pattern::Compiling => true,
            state::Pattern::Ready(_) | state::Pattern::Migemo(_) => {
                !matches!(state.tally, Some(Tally::Known { .. }))
            }
            state::Pattern::Empty | state::Pattern::Bad(_) => false,
        };
        let buffer = self.current().1;
        let cursor = motion::cluster_start(&buffer.display(), step.cursor);
        self.windows[self.selected].selections.move_primary(cursor);
        if recount {
            self.isearch_count(None);
        }
    }

    /// Makes what the string looks for anew, as [`Isearch::rebuild_query`] does, with the
    /// search marked around a secret first if one shows now, so its needle is not cached.
    /// Returns when it began, which the key's time to search counts from.
    pub(super) fn isearch_rebuild(&mut self) -> Instant {
        let secret = self.shows_secret();
        let Some(state) = &mut self.isearch else {
            return Instant::now();
        };
        state.around_secret |= secret;
        state.rebuild_query(&mut self.migemo)
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
