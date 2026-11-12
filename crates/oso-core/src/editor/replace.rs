//! query-replace (ADR 0113): a mode of the selected window, as isearch is, that asks at each
//! match in its scope whether to replace it.
//!
//! Must not depend on: `command` or `keymap`, or the host's window or threads. A search past
//! a key's budget, and the count, run on isearch's worker; `!` replaces a slice a turn, which
//! the host asks for while [`Editor::replace_works`] says so.
//!
//! - The scope is every shown region, or the primary cursor to the end of the buffer, kept in
//!   the buffer as tracked ranges, so that it follows the session's own edits.
//! - Each replacement is an undo step while the session runs, so `u` undoes one, and the
//!   steps join into one when it ends.

use std::collections::VecDeque;
use std::fmt;
use std::ops::Range;
use std::sync::atomic::Ordering;

use super::Editor;
use super::isearch::MatchCells;
use crate::buffer::ReadOnly;
use crate::motion;
use crate::search::{
    self, Counting, Found, KEY_BUDGET, LineTooLong, Needle, Query, Report, SearchJob, SearchReport,
};
use crate::selection::Selection;
use crate::window::{Screen, WindowId};

mod all;
mod answers;
mod state;

pub use state::{Replace, ReplaceAnswer};

/// The most replacements `!` makes in one slice, which ADR 0091's batched pass takes in a few
/// milliseconds.
const ALL_SLICE: usize = 1000;

/// Why query-replace did not begin. Nothing changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplaceError {
    /// The buffer is read-only, for this reason.
    ReadOnly(ReadOnly),
    /// No string to replace, and none from before.
    EmptyString,
    /// The regexp does not compile, for this reason, on one line.
    Regexp(String),
}

impl fmt::Display for ReplaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ReadOnly(ReadOnly::Loading) => "Buffer is still loading",
            Self::ReadOnly(_) => "Buffer is read-only",
            Self::EmptyString => "Empty search string",
            Self::Regexp(why) => why,
        })
    }
}

/// The session and the last pair of strings of each kind, for an empty answer. Its `Debug`
/// prints no string.
#[derive(Default)]
pub(super) struct Replacing {
    session: Option<Replace>,
    /// The last pair given, literal first and regexp second.
    last: [Option<(String, String)>; 2],
}

impl fmt::Debug for Replacing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Replacing")
            .field("session", &self.session)
            .field(
                "last",
                &self.last.iter().map(Option::is_some).collect::<Vec<_>>(),
            )
            .finish()
    }
}

/// The most answers kept while a worker looks for the next match.
const MOST_PENDING: usize = 16;

/// Merges `ranges`, sorted, into ranges apart.
fn merged(mut ranges: Vec<Range<usize>>) -> Vec<Range<usize>> {
    ranges.sort_unstable_by_key(|range| range.start);
    let mut out: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
    for range in ranges {
        match out.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => out.push(range),
        }
    }
    out
}

impl Editor {
    /// The query-replace under way, if any.
    pub fn replace(&self) -> Option<&Replace> {
        self.replacing.session.as_ref()
    }

    /// The last pair of strings given for a replace of this kind, for the first prompt to
    /// offer, and an empty answer to take.
    pub fn last_replace(&self, regexp: bool) -> Option<(&str, &str)> {
        self.replacing.last[usize::from(regexp)]
            .as_ref()
            .map(|(from, to)| (from.as_str(), to.as_str()))
    }

    /// Whether query-replace can begin in the selected window's buffer.
    ///
    /// # Errors
    ///
    /// [`ReplaceError::ReadOnly`] for a buffer that cannot be edited.
    pub fn replace_may_begin(&self) -> Result<(), ReplaceError> {
        match self.current().1.read_only() {
            Some(reason) => Err(ReplaceError::ReadOnly(reason)),
            None => Ok(()),
        }
    }

    /// Begins replacing `from` with `to` in the selected window, at its first match in the
    /// scope. An empty `from` takes the last pair. The search is literal, with smart case,
    /// and the replacement takes the match's case while it folds.
    ///
    /// # Errors
    ///
    /// Why it did not begin. Nothing changes.
    pub fn replace_begin(
        &mut self,
        from: &str,
        to: &str,
        regexp: bool,
    ) -> Result<(), ReplaceError> {
        self.replace_may_begin()?;
        let (from, to) = if from.is_empty() {
            let (from, to) = self.last_replace(regexp).ok_or(ReplaceError::EmptyString)?;
            (from.to_owned(), to.to_owned())
        } else {
            (from.to_owned(), to.to_owned())
        };
        let (needle, case_sensitive) = if regexp {
            let regexp = search::compile_regexp(&from).map_err(ReplaceError::Regexp)?;
            (Needle::Regexp(regexp), search::regexp_smart_case(&from))
        } else {
            let case_sensitive = search::smart_case(&from);
            let query = Query::new(&from, case_sensitive).ok_or(ReplaceError::EmptyString)?;
            (Needle::Literal(query), case_sensitive)
        };
        self.isearch_abandon();
        self.replace_end(false);
        let around_secret = self.shows_secret();
        if !around_secret {
            self.replacing.last[usize::from(regexp)] = Some((from.clone(), to.clone()));
        }
        let id = self.window().buffer();
        let cursor = self.window().selections.primary().cursor;
        // Where an undo of the whole session takes the cursors back to.
        let before = self.window().selections.clone();
        let selections = &self.window().selections;
        let ranges = if selections.mark_active().is_active() {
            merged(selections.iter().map(Selection::range).collect())
        } else {
            std::iter::once(cursor..self.current().1.text().len()).collect()
        };
        self.deactivate_mark();
        let scope = self.new_tracking();
        let buffer = self.buffer_mut(id).ok_or(ReplaceError::EmptyString)?;
        buffer.track(scope, ranges.clone());
        let began = (cursor, buffer.undo_mark());
        let revision = buffer.revision();
        let counting = Counting {
            text: buffer.text().clone(),
            invalid: buffer.invalid_bytes().clone(),
            ranges: ranges.clone(),
        };
        self.replacing.session = Some(Replace {
            from,
            to,
            regexp,
            needle,
            preserve_case: !case_sensitive,
            buffer: id,
            scope,
            current: None,
            stay: None,
            stay_empty: false,
            searching: None,
            pending: VecDeque::new(),
            total: None,
            replaced: 0,
            skipped: 0,
            visits: Vec::new(),
            began,
            before: Some(before),
            revision,
            around_secret,
            all_from: None,
            help: false,
            counting: Some(counting),
            back: 0,
        });
        let start = ranges.first().map_or(cursor, |range| range.start);
        self.replace_find(start);
        // The count runs on the worker, unless a find already took it there.
        if self
            .replace()
            .is_some_and(|state| state.searching.is_none())
        {
            self.replace_job(None, true);
        }
        Ok(())
    }

    /// The scope's ranges, as edits have moved them.
    fn replace_scope(&self) -> Vec<Range<usize>> {
        let Some(state) = self.replace() else {
            return Vec::new();
        };
        self.buffer(state.buffer)
            .and_then(|buffer| buffer.tracked(state.scope))
            .map(<[_]>::to_vec)
            .unwrap_or_default()
    }

    /// Looks for the next match in the scope from `from`, within a key's budget, and goes on
    /// on the worker past it. With none left, the session ends.
    fn replace_find(&mut self, from: usize) {
        let Some(state) = self.replace() else {
            return;
        };
        let (id, needle) = (state.buffer, state.needle.clone());
        let ranges = self.replace_scope();
        let Some(buffer) = self.buffer(id) else {
            return;
        };
        let snapshot = (buffer.text(), buffer.invalid_bytes());
        let never = || false;
        let mut budget = KEY_BUDGET;
        let mut found = None;
        let mut resume = None;
        for range in ranges.iter().filter(|range| range.end > from) {
            let start = range.start.max(from);
            match needle.find_in(snapshot, start..range.end, budget, &never) {
                Ok(Found::Match(range)) => {
                    found = Some(range);
                    break;
                }
                Ok(Found::None) => budget = budget.saturating_sub(range.end - start),
                Ok(Found::Stopped { resume: at }) => {
                    resume = Some(at);
                    break;
                }
                Err(LineTooLong(at)) => {
                    self.replace_line_too_long(at);
                    return;
                }
            }
            if budget == 0 {
                resume = Some(range.end);
                break;
            }
        }
        match (found, resume) {
            (Some(range), _) => self.replace_at(range),
            (None, Some(at)) => {
                let count = self.replace().is_some_and(|state| state.total.is_none());
                if let Some(state) = &mut self.replacing.session {
                    state.searching = Some(at);
                    state.current = None;
                }
                self.replace_job(Some(at), count);
            }
            (None, None) => self.replace_end(true),
        }
    }

    /// Leaves isearch's worker a job over the scope: a find from `find`, and a count.
    fn replace_job(&mut self, find: Option<usize>, count: bool) {
        let Some(state) = self.replace() else {
            return;
        };
        let (id, needle) = (state.buffer, state.needle.clone());
        let ranges = self.replace_scope();
        let generation = self.supersede_search();
        let Some(buffer) = self.buffer(id) else {
            return;
        };
        let snapshot = (buffer.text().clone(), buffer.invalid_bytes().clone());
        // The count is of the scope when the session began, whatever it replaced since.
        let counting = self
            .replace()
            .and_then(|state| state.counting.clone())
            .filter(|_| count);
        let live = (generation, self.search_live.clone());
        let what = (find, counting);
        self.search_job = Some(SearchJob::scoped(snapshot, needle, ranges, what, live));
    }

    /// Asks about `range`: the cursor goes to its end.
    fn replace_at(&mut self, range: Range<usize>) {
        let Some(state) = &mut self.replacing.session else {
            return;
        };
        state.current = Some(range.clone());
        state.searching = None;
        state.stay = None;
        state.back = 0;
        let display = self.current().1.display();
        let cursor = motion::cluster_start(&display, range.end);
        self.windows[self.selected].selections.move_primary(cursor);
        self.follow_cursor();
        self.replace_run_pending();
    }

    /// Takes in what isearch's worker reported for the session. Returns whether what shows
    /// changed.
    pub(super) fn replace_reported(&mut self, report: SearchReport) -> bool {
        let live = self.search_live.load(Ordering::Relaxed);
        let Some(state) = &self.replacing.session else {
            return false;
        };
        let stale = self
            .buffer(state.buffer)
            .is_none_or(|buffer| buffer.revision() != state.revision);
        match report.0 {
            Report::Found { generation, at } if generation == live => {
                let Some(from) = state.searching else {
                    return false;
                };
                if stale {
                    self.replace_find(from);
                    return true;
                }
                match at {
                    Some(range) => self.replace_at(range),
                    None => self.replace_end(true),
                }
                true
            }
            Report::LineTooLong { generation, at } if generation == live => {
                self.replace_line_too_long(at);
                true
            }
            // The count is of the scope when it began, so edits since do not make it stale.
            Report::Counted {
                generation, count, ..
            } if generation == live => {
                if let Some(state) = &mut self.replacing.session {
                    state.total = Some(count.total);
                    state.counting = None;
                }
                true
            }
            _ => false,
        }
    }

    /// Ends the session, keeping what it replaced, as one undo step. With `say`, the echo
    /// area says how many it replaced, and only it while a buffer that may hold secrets is
    /// around. The place it began goes onto the mark ring if the cursor moved, as Emacs's
    /// `push-mark` does.
    pub(super) fn replace_end(&mut self, say: bool) {
        let Some(state) = self.replacing.session.take() else {
            return;
        };
        self.supersede_search();
        if let Some(buffer) = self.buffer_mut(state.buffer) {
            buffer.untrack(state.scope);
            buffer.join_undo_since(state.began.1, state.before.clone());
        }
        if self.window().buffer() == state.buffer {
            let selections = &mut self.windows[self.selected].selections;
            if selections.primary().cursor != state.began.0 {
                selections.push_primary_mark(state.began.0);
            }
        }
        if !say {
            return;
        }
        let plural = |n: usize| if n == 1 { "" } else { "s" };
        let mut said = format!(
            "Replaced {} occurrence{}",
            state.replaced,
            plural(state.replaced)
        );
        if state.skipped > 0 {
            said.push_str(&format!(" (skipped {})", state.skipped));
        }
        if state.around_secret || self.shows_secret() {
            self.echo(said);
        } else {
            self.message(said);
        }
    }

    /// Ends the session as `RET` does. Returns whether one was under way.
    pub fn replace_exit(&mut self) -> bool {
        let was = self.replace().is_some();
        self.replace_end(true);
        was
    }

    /// Ends the session quietly, as a prompt opening or the window showing another buffer
    /// does.
    pub(super) fn replace_abandon(&mut self) {
        self.replace_end(false);
    }

    /// `C-g`'s step for the session: it ends, keeping what it replaced, as Emacs's does.
    pub(super) fn replace_quit(&mut self) -> bool {
        let was = self.replace().is_some();
        self.replace_end(false);
        was
    }

    /// Notes that a buffer that may hold secrets is shown while the session is on.
    pub(super) fn replace_note_secret(&mut self) {
        if let Some(state) = &mut self.replacing.session {
            state.around_secret = true;
        }
    }

    /// The echo line of a query-replace under way, after what `out` holds.
    pub(super) fn write_replace_line(&self, out: &mut String) {
        if let Some(state) = self.replace() {
            state.write_line(out);
        }
    }

    /// The match asked about and the others in the scope that the view shows, as cells, for
    /// the renderer, as isearch's are, in place of what `cells` held. With no query-replace
    /// under way, `cells` stays as it is.
    pub fn replace_cells(&self, id: WindowId, screen: &Screen, cells: &mut MatchCells) {
        // With none under way, the cells stay isearch's.
        let Some(state) = self.replace() else {
            return;
        };
        cells.current.clear();
        cells.others.clear();
        if state.all_from.is_some() {
            return;
        }
        let scope = self.replace_scope();
        let current = state.current.as_ref().or(state.stay.as_ref());
        let query = (&state.needle, current, Some(scope.as_slice()));
        self.match_cells(id, state.buffer, screen, query, cells);
    }

    /// Ends the session at a line a regexp cannot be matched in, which starts at `at`.
    fn replace_line_too_long(&mut self, at: usize) {
        let row = self.current().1.text().offset_to_point(at).row + 1;
        let limit = search::MAX_LINE / (1024 * 1024);
        self.replace_end(false);
        self.echo(format!(
            "Regexp search stops at line {row}: it is longer than {limit} MiB"
        ));
    }

    /// The replacement for the match at `range`: the session's, with a regexp's groups
    /// expanded, in the match's case while the search folds.
    fn replacement_for(&self, range: &Range<usize>) -> Option<String> {
        let state = self.replace()?;
        let text = self.buffer(state.buffer)?.text();
        let to = state.needle.replacement(text, range, &state.to);
        if !state.preserve_case {
            return Some(to);
        }
        let matched: String = text.chunks_in_range(range.clone()).collect();
        Some(search::casified(&matched, &to).into_owned())
    }

    /// A new id for ranges tracked in a buffer.
    fn new_tracking(&mut self) -> crate::buffer::Tracking {
        self.next_tracking()
    }
}

/// Where the next search starts after `range` in `text`: its end, or past an empty match,
/// the next character.
fn after(text: &crate::rope::Rope, range: &Range<usize>) -> usize {
    search::after(text, range)
}

impl Editor {
    /// Where the session's next search starts after `range`, as [`after`] says.
    fn next_from(&self, range: &Range<usize>) -> usize {
        self.replace()
            .and_then(|state| self.buffer(state.buffer))
            .map_or(range.end, |buffer| after(buffer.text(), range))
    }
}

#[cfg(test)]
mod tests;
