//! The keys that add cursors (ADR 0091): at the next or previous occurrence of the region
//! or the word, skipping one, at every one, and on each line of the region; the query a
//! window keeps for a run of them; and the part of a search a worker does.
//!
//! Must not depend on: `command` or `keymap`; the commands call these.
//!
//! - The query is the primary cursor's region, or the word at each cursor, which the first
//!   key selects. It can be secret text: its `Debug` prints none, nothing logs it, and
//!   `oso dev` does not carry it.
//! - Each command reads [`KEY_BUDGET`] on the UI thread, whatever its count: the text the
//!   matcher reads and what checking each match costs. It leaves the rest to isearch's
//!   worker, in `worker`.
//! - An occurrence starts and ends on display clusters, where a cursor can be.

use std::fmt;
use std::ops::{Range, RangeInclusive};

use super::Editor;
use crate::buffer::BufferId;
use crate::display_map::{DisplayPoint, DisplaySnapshot};
use crate::motion::{self, WalkBudget};
use crate::search::{KEY_BUDGET, MAX_QUERY_CHARS, Matched, Needle, Query};
use crate::selection::{Goal, MAX_CURSORS, MarkActive, Selection, SelectionSet};
use crate::sum_tree::Bias;
use crate::window::WindowId;
use crate::word;

mod find;
#[cfg(test)]
mod isearch_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod word_tests;
mod worker;

pub use find::Toward;
use find::{All, Text, Wanted};

/// Why a key that adds cursors added none, or no more.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorError {
    /// No region shows, and no word is at the primary cursor.
    NoWord,
    /// The region is longer than a search string may be.
    TooLong,
    /// There is no occurrence that way, or no more of them.
    NoMore,
    /// The window would have more than [`MAX_CURSORS`].
    TooMany,
    /// The buffer holds this many occurrences, more than [`MAX_CURSORS`].
    TooManyMatches(usize),
    /// There is one cursor, and none to remove.
    OnlyOne,
    /// The buffer is still loading, and a cursor on every occurrence would miss some.
    Loading,
    /// The primary cursor has no mark.
    NoMark,
    /// isearch has no current match: it fails, or a worker still looks for one.
    NoCurrentMatch,
    /// isearch has not counted its matches yet, which a cursor on every one waits for.
    Counting,
}

impl fmt::Display for CursorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoWord => f.write_str("No word at point"),
            Self::TooLong => f.write_str("Region too long to match"),
            Self::NoMore => f.write_str("No more matches"),
            Self::TooMany => write!(f, "At most {MAX_CURSORS} cursors"),
            Self::TooManyMatches(total) => {
                write!(f, "{total} matches; at most {MAX_CURSORS} cursors")
            }
            Self::OnlyOne => f.write_str("Only one cursor"),
            Self::Loading => f.write_str("Buffer is still loading"),
            Self::NoMark => f.write_str("The mark is not set now"),
            Self::NoCurrentMatch => f.write_str("No current match"),
            Self::Counting => f.write_str("Still counting matches"),
        }
    }
}

impl std::error::Error for CursorError {}

/// The query a window keeps for a run of the keys that add cursors, while it has several
/// cursors and its text stays, and a search a worker does for it.
pub(super) struct Marking {
    window: WindowId,
    buffer: BufferId,
    revision: u64,
    wanted: Wanted,
    pending: Option<Pending>,
}

/// A search a worker does for a key that adds cursors.
struct Pending {
    generation: u64,
    ask: Ask,
    /// The window's cursors when it was asked, which must still be its own.
    selections: SelectionSet,
    /// How long the text was if it was loading, as a load only appends.
    loading: Option<usize>,
}

/// What a search for cursors is for.
#[derive(Clone, Copy)]
enum Ask {
    /// To add up to `most` cursors.
    Add { toward: Toward, most: usize },
    /// To move the cursor at that end to the next occurrence.
    Skip(Toward),
    /// To put a cursor on every occurrence.
    All,
}

impl Editor {
    /// Adds a cursor, `count` times, at the next occurrence after the last cursor, or the
    /// previous before the first, of the primary cursor's region, or of the word at each
    /// cursor, which the first key selects. The new one becomes the primary. A negative
    /// `count` removes that many cursors from that end, and 0 skips, as
    /// multiple-cursors.el does.
    ///
    /// # Errors
    ///
    /// A [`CursorError`], after adding what it found when it found fewer.
    pub fn mark_like_this(&mut self, toward: Toward, count: isize) -> Result<(), CursorError> {
        self.drop_cursor_search();
        match count {
            0 => self.skip_like_this(toward),
            ..0 => self.remove_cursors(toward, count.unsigned_abs()),
            _ => self.find_cursors(toward, count.unsigned_abs(), false),
        }
    }

    /// Moves the last cursor to the next occurrence after it, or the first to the one
    /// before it.
    ///
    /// # Errors
    ///
    /// A [`CursorError`], and nothing changes.
    pub fn skip_like_this(&mut self, toward: Toward) -> Result<(), CursorError> {
        self.drop_cursor_search();
        self.find_cursors(toward, 1, true)
    }

    /// Puts a cursor on every occurrence in the buffer, the primary staying on its own.
    ///
    /// # Errors
    ///
    /// A [`CursorError`], and nothing changes but the words the query was taken from.
    pub fn mark_all_like_this(&mut self) -> Result<(), CursorError> {
        self.drop_cursor_search();
        if self.current().1.loading().is_some() {
            return Err(CursorError::Loading);
        }
        let wanted = self.wanted()?;
        self.all_cursors(wanted)
    }

    /// Puts a cursor on every occurrence of `wanted`, as far as one key reads, and leaves
    /// the rest to the worker.
    fn all_cursors(&mut self, wanted: Wanted) -> Result<(), CursorError> {
        let text = self.cursor_text();
        let mut left = KEY_BUDGET;
        match text.all(&wanted, &mut left, &|| false) {
            All::Done(matched) => self.put_all(&matched),
            All::Stopped => {
                self.ask_worker(Ask::All, 0, (text, wanted));
                Ok(())
            }
        }
    }

    /// Puts one cursor on each line from the mark's to the primary cursor's, at the
    /// primary cursor's goal column, or at a shorter line's end, as multiple-cursors.el's
    /// `mc/edit-lines` does. The regions hide, and the primary stays on its line.
    ///
    /// # Errors
    ///
    /// [`CursorError::NoMark`] or [`CursorError::TooMany`], and nothing changes.
    pub fn edit_lines(&mut self) -> Result<(), CursorError> {
        self.drop_cursor_search();
        let (window, buffer) = self.current();
        let primary = *window.selections.primary();
        let mark = primary.mark.ok_or(CursorError::NoMark)?;
        let display = buffer.display();
        let at = display.offset_to_display_point(primary.cursor);
        let rows = lines_between(display.offset_to_display_point(mark).row, at.row);
        if rows.clone().count() > MAX_CURSORS {
            return Err(CursorError::TooMany);
        }
        let goal = primary.goal.unwrap_or(Goal::Column(at.column));
        let parts: Vec<Selection> =
            rows.clone()
                .map(|row| Selection {
                    cursor: match goal {
                        Goal::Column(column) => display
                            .display_point_to_offset(DisplayPoint::new(row, column), Bias::Left),
                        Goal::LineEnd => motion::line_end(&display, row),
                    },
                    mark: None,
                    goal: Some(goal),
                })
                .collect();
        let ring = *window.selections.mark_ring();
        let primary = at.row - rows.start();
        let Some(set) = SelectionSet::from_parts(parts, primary) else {
            return Err(CursorError::NoMark);
        };
        self.windows[self.selected].selections = set.with_ring(ring);
        self.marking = None;
        Ok(())
    }

    /// `C->` or `C-<` in isearch, `toward` either way, or `C-c C-<` with none: ends the
    /// search as `RET` does, with its current match, on whole clusters, as the primary's
    /// region in the search's direction, and goes on as those keys do with the search's
    /// string and case, not only whole words.
    ///
    /// # Errors
    ///
    /// [`CursorError::NoCurrentMatch`]; and for every match, while the buffer loads, the
    /// count is not known, or it is over [`MAX_CURSORS`]. The search then goes on. Otherwise,
    /// after it ended, what the key says.
    pub fn isearch_mark(&mut self, toward: Option<Toward>) -> Result<(), CursorError> {
        self.drop_cursor_search();
        let (needle, current, total, backward) = self
            .isearch_current_match()
            .ok_or(CursorError::NoCurrentMatch)?;
        // Once, for every key of the run, which keeps this copy: no key builds them.
        if let Needle::Regexp(regexp) = &needle {
            regexp.build_cut();
        }
        if toward.is_none() {
            if self.current().1.loading().is_some() {
                return Err(CursorError::Loading);
            }
            match total {
                None => return Err(CursorError::Counting),
                Some(total) if total > MAX_CURSORS => {
                    return Err(CursorError::TooManyMatches(total));
                }
                Some(_) => {}
            }
        }
        let display = self.current().1.display();
        let start = motion::cluster_start(&display, current.start);
        let end = motion::cluster_end(&display, current.end);
        self.isearch_exit();
        let (mark, cursor) = if backward { (end, start) } else { (start, end) };
        let selections = &mut self.windows[self.selected].selections;
        selections.move_primary(cursor);
        selections.push_primary_mark(mark);
        selections.set_mark_active(MarkActive::Active);
        let wanted = Wanted {
            needle,
            whole_word: false,
        };
        let (window, buffer) = self.current();
        self.marking = Some(Marking {
            window: window.id(),
            buffer: window.buffer(),
            revision: buffer.revision(),
            wanted: wanted.clone(),
            pending: None,
        });
        match toward {
            Some(toward) => self.found_cursors(wanted, toward, 1, false),
            None => self.all_cursors(wanted),
        }
    }

    /// Leaves the selected window the primary cursor alone, if it has more, as the last
    /// step of `C-g`.
    pub(super) fn quit_cursors(&mut self) -> bool {
        let selections = &mut self.windows[self.selected].selections;
        if selections.len() < 2 {
            return false;
        }
        selections.keep_primary();
        self.drop_cursor_search();
        self.marking = None;
        true
    }

    /// Adds `most` cursors `toward` either way, or with `skip` moves the one at that end.
    fn find_cursors(&mut self, toward: Toward, most: usize, skip: bool) -> Result<(), CursorError> {
        let wanted = self.wanted()?;
        self.found_cursors(wanted, toward, most, skip)
    }

    /// Adds `most` occurrences of `wanted` `toward` either way, or with `skip` moves the one
    /// at that end.
    fn found_cursors(
        &mut self,
        wanted: Wanted,
        toward: Toward,
        most: usize,
        skip: bool,
    ) -> Result<(), CursorError> {
        let set = &self.windows[self.selected].selections;
        if !skip && set.len().saturating_add(most) > MAX_CURSORS {
            return Err(CursorError::TooMany);
        }
        let from = match toward {
            Toward::Next => set.as_slice().last().map_or(0, |last| last.range().end),
            Toward::Previous => set
                .as_slice()
                .first()
                .map_or(0, |first| first.range().start),
        };
        let text = self.cursor_text();
        let mut left = KEY_BUDGET;
        let mut found = Vec::new();
        let resume = text.several(
            &wanted,
            (toward, from),
            (most, &mut left),
            &|| false,
            &mut found,
        );
        if !found.is_empty() {
            self.put_found(toward, skip, &found)?;
        }
        match resume {
            Some(at) => {
                let ask = if skip {
                    Ask::Skip(toward)
                } else {
                    Ask::Add {
                        toward,
                        most: most - found.len(),
                    }
                };
                self.ask_worker(ask, at, (text, wanted));
                Ok(())
            }
            None if found.len() < most => Err(CursorError::NoMore),
            None => Ok(()),
        }
    }

    /// Removes up to `count` cursors from the end `toward` says, leaving one at least.
    fn remove_cursors(&mut self, toward: Toward, count: usize) -> Result<(), CursorError> {
        let set = &self.windows[self.selected].selections;
        if set.len() < 2 {
            return Err(CursorError::OnlyOne);
        }
        let active = set.mark_active();
        let mut parts = set.as_slice().to_vec();
        let removed = count.min(parts.len() - 1);
        let kept = parts.len() - removed;
        let primary = match toward {
            Toward::Next => {
                parts.truncate(kept);
                set.primary_index().min(kept - 1)
            }
            Toward::Previous => {
                parts.drain(..removed);
                set.primary_index().saturating_sub(removed)
            }
        };
        self.replace_cursors(parts, primary, active);
        Ok(())
    }

    /// Adds a cursor on each of `ranges`, in the direction of the primary's region, the
    /// last of them primary, after removing the cursor at the end `toward` says if `skip`.
    fn put_found(
        &mut self,
        toward: Toward,
        skip: bool,
        ranges: &[Range<usize>],
    ) -> Result<(), CursorError> {
        if ranges.is_empty() {
            return Err(CursorError::NoMore);
        }
        let set = &self.windows[self.selected].selections;
        let mut parts = set.as_slice().to_vec();
        if skip {
            match toward {
                Toward::Next => drop(parts.pop()),
                Toward::Previous => drop(parts.remove(0)),
            }
        }
        if parts.len() + ranges.len() > MAX_CURSORS {
            return Err(CursorError::TooMany);
        }
        let forward = forward(set.primary());
        parts.extend(ranges.iter().map(|range| oriented(range, forward)));
        let primary = parts.len() - 1;
        self.replace_cursors(parts, primary, MarkActive::Active);
        Ok(())
    }

    /// Puts a cursor on each of `matched`, the primary on the one at its own region.
    ///
    /// When no occurrence covers the primary's own region, as a search's current match
    /// widened to whole clusters may not, that region is one more cursor, the primary.
    fn put_all(&mut self, matched: &Matched) -> Result<(), CursorError> {
        let own = *self.windows[self.selected].selections.primary();
        let mut ranges = matched.ranges.clone();
        let mut total = matched.total;
        let covered = |range: &Range<usize>| range.contains(&own.cursor) || range.end == own.cursor;
        let primary = match ranges.iter().position(covered) {
            Some(index) => index,
            None if own.mark.is_some_and(|mark| mark != own.cursor) => {
                let region = own.range();
                let index = ranges.partition_point(|range| range.start < region.start);
                ranges.insert(index, region);
                total += 1;
                index
            }
            None => 0,
        };
        if total > MAX_CURSORS {
            return Err(CursorError::TooManyMatches(total));
        }
        if ranges.is_empty() {
            return Err(CursorError::NoMore);
        }
        let forward = forward(&own);
        let parts = ranges
            .iter()
            .map(|range| oriented(range, forward))
            .collect();
        self.replace_cursors(parts, primary, MarkActive::Active);
        Ok(())
    }

    /// Adds a cursor with no mark at `offset` of the window at `index`, as a jump's `c` does
    /// (ADR 0116): the primary stays, the regions show as they did, and a run of the keys that
    /// add cursors starts again.
    ///
    /// # Errors
    ///
    /// [`CursorError::TooMany`] with [`MAX_CURSORS`] there already, and nothing changes.
    pub(super) fn add_cursor_in(&mut self, index: usize, offset: usize) -> Result<(), CursorError> {
        let set = &self.windows[index].selections;
        if set.len() >= MAX_CURSORS {
            return Err(CursorError::TooMany);
        }
        let mut parts = set.as_slice().to_vec();
        parts.push(Selection::at(offset));
        let (primary, active) = (set.primary_index(), set.mark_active());
        self.drop_cursor_search();
        self.marking = None;
        self.replace_cursors_in(index, parts, primary, active);
        Ok(())
    }

    /// Makes `parts` the selected window's cursors, with `primary` primary and its ring
    /// kept, and the regions as `active` says.
    fn replace_cursors(&mut self, parts: Vec<Selection>, primary: usize, active: MarkActive) {
        self.replace_cursors_in(self.selected, parts, primary, active);
    }

    /// [`Editor::replace_cursors`] in the window at `index`.
    fn replace_cursors_in(
        &mut self,
        index: usize,
        parts: Vec<Selection>,
        primary: usize,
        active: MarkActive,
    ) {
        let selections = &mut self.windows[index].selections;
        let Some(set) = SelectionSet::from_parts(parts, primary) else {
            return;
        };
        let mut set = set.with_ring(*selections.mark_ring());
        set.set_mark_active(active);
        *selections = set;
    }

    /// What the keys that add cursors look for: the query the window keeps, or a new one
    /// from the primary's region, or from the word at each cursor, which it selects.
    fn wanted(&mut self) -> Result<Wanted, CursorError> {
        let (window, buffer) = self.current();
        let me = window.id();
        let (id, revision) = (window.buffer(), buffer.revision());
        let selections = &window.selections;
        if let Some(marking) = &self.marking
            && (marking.window, marking.buffer, marking.revision) == (me, id, revision)
            && selections.len() > 1
        {
            return Ok(marking.wanted.clone());
        }
        let range = selections.primary().range();
        let (range, whole_word) = if selections.mark_active().is_active() && !range.is_empty() {
            (range, false)
        } else {
            (self.select_words()?, true)
        };
        // A character takes at most 4 bytes, so a longer region needs no copy to refuse.
        if range.len() > MAX_QUERY_CHARS * 4 {
            return Err(CursorError::TooLong);
        }
        let string = self.current().1.text().slice(range).to_string();
        if string.chars().count() > MAX_QUERY_CHARS {
            return Err(CursorError::TooLong);
        }
        let query = Query::new(&string, true).ok_or(CursorError::NoWord)?;
        let wanted = Wanted {
            needle: Needle::Literal(query),
            whole_word,
        };
        let revision = self.current().1.revision();
        self.marking = Some(Marking {
            window: me,
            buffer: id,
            revision,
            wanted: wanted.clone(),
            pending: None,
        });
        Ok(wanted)
    }

    /// Selects the word at each cursor, and shows the regions. Returns the primary's.
    fn select_words(&mut self) -> Result<Range<usize>, CursorError> {
        let display = self.current().1.display();
        let selections = &mut self.windows[self.selected].selections;
        // The cursors share one command's reading, the primary's first, as its word is the
        // query; one it runs out on selects nothing (ADR 0101).
        let mut budget = WalkBudget::for_cursors(selections.len());
        let primary = word_around(&display, selections.primary().cursor, &mut budget)
            .ok_or(CursorError::NoWord)?;
        let (primary_index, mut index) = (selections.primary_index(), 0);
        selections.update(|selection| {
            let word = if index == primary_index {
                Some(primary.clone())
            } else {
                word_around(&display, selection.cursor, &mut budget)
            };
            index += 1;
            let range = word.unwrap_or(selection.cursor..selection.cursor);
            selection.mark = Some(range.start);
            selection.move_to(range.end);
            Ok::<(), ()>(())
        });
        selections.set_mark_active(MarkActive::Active);
        Ok(primary)
    }

    /// The selected buffer's text, as a search for cursors on the UI thread reads it, in O(1).
    fn cursor_text(&self) -> Text {
        let buffer = self.current().1;
        Text {
            rope: buffer.text().clone(),
            invalid: buffer.invalid_bytes().clone(),
            display: buffer.display(),
            until: self.key_until(std::time::Instant::now()),
            on_key: true,
        }
    }
}

/// The rows from `a` to `b`, in order.
fn lines_between(a: usize, b: usize) -> RangeInclusive<usize> {
    a.min(b)..=a.max(b)
}

/// Whether a selection's region goes forward, from its mark to its cursor.
fn forward(selection: &Selection) -> bool {
    selection.mark.is_none_or(|mark| mark <= selection.cursor)
}

/// A selection over `range`, its cursor at the end if `forward`, and at the start if not.
fn oriented(range: &Range<usize>, forward: bool) -> Selection {
    let (mark, cursor) = if forward {
        (range.start, range.end)
    } else {
        (range.end, range.start)
    };
    Selection {
        cursor,
        mark: Some(mark),
        goal: None,
    }
}

/// The word around `offset`: the one it is in or at the start of, or the one it ends,
/// reading from `budget`. None if the budget runs out on it.
fn word_around(
    display: &DisplaySnapshot,
    offset: usize,
    budget: &mut WalkBudget,
) -> Option<Range<usize>> {
    let end = if word::word_at(display, offset) {
        word::forward_word(display, offset, budget)
    } else {
        let before = motion::prev_cluster(display, offset)?;
        if !word::word_at(display, before) {
            return None;
        }
        offset
    };
    let start = word::backward_word(display, end, budget);
    (start < end && !budget.is_spent()).then_some(start..end)
}
