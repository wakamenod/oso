//! The completion popup at point (ADR 0138, Completion in the buffer, step 9), as Corfu's: the
//! completions a server offered for the word before the selected window's primary cursor,
//! matched against that word as it is typed, at most [`POPUP_ROWS`] shown under or over its
//! row, and the selected one inserted at the cursors that share the word.
//!
//! Must not depend on: `command` or `keymap`, or the host's servers, threads or I/O; the host
//! asks the server and hands over the answer, and the popup's keys are the dispatcher's.
//!
//! - The word starts where the server's first edit does, else at the identifier characters
//!   before the cursor: whole grapheme clusters whose first character is alphanumeric or `_`.
//! - What is matched is an item's `filterText`, else its label, by ADR 0076's matcher, in the
//!   server's order; a row shows the label, then its kind and detail as its annotation.
//! - The popup closes once the cursor leaves the word, another window or buffer is shown, a
//!   prompt opens, or nothing matches; and on any command not its own (the dispatcher's).
//! - Inserting replaces the word, and what the server's edit covers past it, with the item's
//!   text, at the primary cursor and at each other cursor whose word before it is the same, in
//!   one undo step.
//! - Nothing of an item reaches a log; `Debug` gives counts.

use std::fmt;
use std::ops::Range;
use std::sync::Arc;

use unicode_segmentation::UnicodeSegmentation;

use crate::buffer::{Buffer, BufferId, Replacement};
use crate::complete::{Candidate, Candidates};
use crate::frame::{FloatRegions, at_point};
use crate::selection::{OffsetEdit, follow_offset};
use crate::sum_tree::Bias;
use crate::window::Screen;
use crate::window::WindowId;

use super::super::Editor;
use super::jumps::AskAt;

/// The most rows of completions the popup shows.
pub const POPUP_ROWS: usize = 10;

/// The most columns of text the popup shows, and no more than half the window's.
pub const POPUP_COLUMNS: usize = 60;

/// The longest word before the cursor looked at, in bytes: past it, the popup closes.
const WORD_BYTES_MAX: usize = 256;

/// A completion as the popup shows and inserts it: its label, its kind and detail as one
/// annotation, what is matched, the text it inserts, and the range of the answer's text it
/// replaces, if the server gave one. `Debug` says nothing of it.
pub struct PopupItem {
    pub label: Box<str>,
    pub annotation: Box<str>,
    pub filter: Option<Box<str>>,
    pub text: Box<str>,
    pub range: Option<Range<usize>>,
}

impl fmt::Debug for PopupItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PopupItem").finish_non_exhaustive()
    }
}

/// What the host hands over of an answer: where it was asked, the revision of the text its
/// items' ranges are in, and its items in the server's order.
#[derive(Debug)]
pub struct PopupAnswer {
    pub at: AskAt,
    pub revision: u64,
    pub items: Vec<PopupItem>,
}

/// The open popup. `Debug` gives counts.
pub struct Popup {
    window: WindowId,
    pub(in crate::editor) buffer: BufferId,
    /// Where the word starts, followed through each edit, as the items' ranges are.
    start: usize,
    /// What each item inserts, and the range it replaces.
    inserts: Vec<(Box<str>, Option<Range<usize>>)>,
    /// What is matched, by item.
    matched: Candidates,
    /// What each row shows, by item.
    shown: Arc<[Candidate]>,
    /// The selection was moved, so that `RET` inserts.
    moved: bool,
}

impl fmt::Debug for Popup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Popup")
            .field("items", &self.inserts.len())
            .field("matches", &self.matched.len())
            .finish_non_exhaustive()
    }
}

impl Popup {
    /// How many items match the word.
    pub fn len(&self) -> usize {
        self.matched.len()
    }

    /// Whether none matches.
    pub fn is_empty(&self) -> bool {
        self.matched.is_empty()
    }

    /// Whether the selection was moved since it opened, so that `RET` inserts it.
    pub fn moved(&self) -> bool {
        self.moved
    }

    /// The rows shown, at most [`POPUP_ROWS`] from the first in view: each item's label and
    /// annotation, and whether it is selected.
    pub fn rows(&self) -> impl Iterator<Item = (bool, &Candidate)> {
        let selected = self.matched.selected();
        self.matched
            .shown(POPUP_ROWS)
            .filter_map(move |(place, _)| {
                let item = self.matched.item_at(place)?;
                Some((selected == Some(place), self.shown.get(item)?))
            })
    }

    /// The parts of a row's label that matched the word, as byte ranges, into `out`.
    pub fn highlights(&self, row: &Candidate, out: &mut Vec<Range<usize>>) {
        self.matched.query().highlights(row, out);
    }
}

impl Editor {
    /// The completion popup, while one is open.
    pub fn popup(&self) -> Option<&Popup> {
        self.servers.popup.as_ref()
    }

    /// Opens the popup for `answer`, if nothing moved since it was asked: the selected window
    /// shows its buffer at its revision with the primary cursor where it was, no prompt is
    /// open, and the buffer does not hold secrets. `Err` says why it did not open: with nothing
    /// to show, `No completions`; otherwise it is stale, and says nothing.
    ///
    /// # Errors
    ///
    /// `Some("No completions")`, or `None` for a stale answer.
    pub fn open_popup(&mut self, answer: PopupAnswer) -> Result<(), Option<&'static str>> {
        let (window, buffer) = self.current();
        let cursor = window.selections.primary().cursor;
        let stale = window.buffer() != answer.at.buffer
            || buffer.revision() != answer.at.revision
            || cursor != answer.at.offset
            || self.prompt().is_some()
            || buffer.is_secret();
        if stale {
            return Err(None);
        }
        if answer.items.is_empty() {
            return Err(Some("No completions"));
        }
        let (window_id, buffer_id) = (window.id(), window.buffer());
        // The items' ranges in the text now: a range that no longer maps is dropped.
        let now = |range: Range<usize>| {
            let start = moved_to(buffer, answer.revision, range.start)?;
            Some(start..moved_to(buffer, answer.revision, range.end)?.max(start))
        };
        let start = answer
            .items
            .iter()
            .find_map(|item| item.range.clone())
            .and_then(now)
            .map(|range| range.start)
            .filter(|start| *start <= cursor && cursor - start <= WORD_BYTES_MAX)
            .unwrap_or_else(|| word_start(buffer, cursor));
        let mut inserts = Vec::with_capacity(answer.items.len());
        let mut matched = Vec::with_capacity(answer.items.len());
        let mut shown = Vec::with_capacity(answer.items.len());
        for item in answer.items {
            let filter = item.filter.unwrap_or_else(|| item.label.clone());
            matched.push(Candidate::new(filter, "", ""));
            shown.push(Candidate::new(item.label, "", item.annotation));
            inserts.push((item.text, item.range.and_then(now)));
        }
        let mut popup = Popup {
            window: window_id,
            buffer: buffer_id,
            start,
            inserts,
            matched: Candidates::in_given_order(matched.into()),
            shown: shown.into(),
            moved: false,
        };
        let word = word_text(buffer, start, cursor);
        popup.matched.set_input_with(&word, None);
        if popup.matched.is_empty() {
            return Err(Some("No completions"));
        }
        self.servers.popup = Some(popup);
        Ok(())
    }

    /// Moves the popup's selection `by` rows, from the last to the first and back, as
    /// `corfu-cycle` does.
    pub fn popup_move(&mut self, by: isize) {
        let Some(popup) = &mut self.servers.popup else {
            return;
        };
        let len = popup.matched.len();
        if len == 0 {
            return;
        }
        let at = popup.matched.selected().unwrap_or(0);
        let len = isize::try_from(len).unwrap_or(isize::MAX);
        let to = (isize::try_from(at).unwrap_or(0) + by).rem_euclid(len);
        popup.matched.select(usize::try_from(to).unwrap_or(0));
        popup.matched.keep_in_view(POPUP_ROWS);
        popup.moved = true;
    }

    /// Closes the popup, if one is open.
    pub fn close_popup(&mut self) {
        self.servers.popup = None;
    }

    /// Inserts the popup's selected item and closes it: the word, and what the item's edit
    /// covers past it, replaced at the primary cursor, and the word at each other cursor whose
    /// word before it is the same, in one edit.
    ///
    /// # Errors
    ///
    /// The edit's [`crate::buffer::EditError`], or `None` with no popup open or nothing
    /// selected.
    pub fn popup_insert(&mut self) -> Result<(), Option<crate::buffer::EditError>> {
        self.follow_popup();
        let popup = self.servers.popup.take().ok_or(None)?;
        let item = popup.matched.selected_item().ok_or(None)?;
        let (text, range) = popup.inserts.get(item).ok_or(None)?;
        let (window, buffer) = self.current();
        let cursor = window.selections.primary().cursor;
        let start = popup.start;
        let word = word_text(buffer, start, cursor);
        // The server's range, and at least the word.
        let (from, to) = range.as_ref().map_or((start, cursor), |range| {
            (range.start.min(start), range.end.max(cursor))
        });
        let mut edits = vec![Replacement {
            range: from..to,
            text,
        }];
        let primary = window.selections.primary_index();
        for (index, selection) in window.selections.as_slice().iter().enumerate() {
            let at = selection.cursor;
            if index == primary || word.is_empty() || at < word.len() {
                continue;
            }
            let before = at - word.len();
            if word_text(buffer, before, at) == word && word_start(buffer, at) == before {
                edits.push(Replacement {
                    range: before..at,
                    text,
                });
            }
        }
        let id = window.buffer();
        self.apply_edits(id, &edits).map_err(Some)
    }

    /// After a command: the popup follows the word as it is typed, matching it again, and
    /// closes once the cursor left it, another window or buffer shows, a prompt opened, or
    /// nothing matches.
    pub fn follow_popup(&mut self) {
        let Some(popup) = &self.servers.popup else {
            return;
        };
        let (window, buffer) = self.current();
        let cursor = window.selections.primary().cursor;
        let start = (window.id() == popup.window && window.buffer() == popup.buffer)
            .then_some(popup.start)
            .filter(|start| {
                *start <= cursor
                    && cursor - start <= WORD_BYTES_MAX
                    && word_start(buffer, cursor) <= *start
            });
        let (Some(start), None) = (start, self.prompt()) else {
            self.servers.popup = None;
            return;
        };
        let word = word_text(buffer, start, cursor);
        let Some(popup) = &mut self.servers.popup else {
            return;
        };
        popup.matched.set_input_with(&word, None);
        if popup.matched.is_empty() {
            self.servers.popup = None;
        }
    }

    /// Where the popup goes over the selected window's text area, as `screen` shows the window
    /// ([`Editor::screen`]): under or over the word's row, its text from the word's start, as
    /// many rows as match, at most [`POPUP_ROWS`], and as wide as the rows shown need, at most
    /// [`POPUP_COLUMNS`] and half the area.
    pub fn popup_regions(&self, screen: &Screen) -> Option<FloatRegions> {
        let popup = self.popup()?;
        let (window, buffer) = self.current();
        let area = self.tiled_regions().text;
        let display = buffer.display();
        let start = popup.start;
        let anchor = |offset| {
            let point = display.offset_to_display_point(offset);
            crate::frame::cursor_rect(area, screen, point, 1)
        };
        let at = anchor(start).or_else(|| anchor(window.selections.primary().cursor))?;
        let widest = popup
            .rows()
            .map(|(_, row)| {
                let label = crate::minibuffer::cells_of(row.text());
                let note = crate::minibuffer::cells_of(row.annotation());
                label + if note > 0 { 2 + note } else { 0 }
            })
            .max()
            .unwrap_or(0);
        let columns = widest.clamp(1, POPUP_COLUMNS.min((area.columns / 2).max(1)));
        let rows = popup.len().min(POPUP_ROWS);
        at_point(area, (at.row, at.column), (rows, columns))
    }
}

impl Editor {
    /// Moves the popup's word and its items' ranges in `buffer` through an edit's `steps`, as
    /// the cursors move.
    pub(in crate::editor) fn popup_follows(&mut self, buffer: BufferId, steps: &[Vec<OffsetEdit>]) {
        let Some(popup) = &mut self.servers.popup else {
            return;
        };
        if popup.buffer != buffer {
            return;
        }
        let follow = |offset: usize, bias| {
            (steps.iter()).fold(offset, |offset, edits| follow_offset(offset, edits, bias))
        };
        popup.start = follow(popup.start, Bias::Left);
        for (_, range) in &mut popup.inserts {
            if let Some(range) = range {
                *range = follow(range.start, Bias::Left)..follow(range.end, Bias::Left);
            }
        }
    }
}

/// `offset` in `buffer`'s text at `revision` moved through its edits since, as the cursors
/// move; `None` once its log no longer reaches back there.
fn moved_to(buffer: &Buffer, revision: u64, offset: usize) -> Option<usize> {
    if buffer.revision() == revision {
        return Some(offset.min(buffer.text().len()));
    }
    let splices = buffer.edits_since(revision)?;
    let mut edits: Vec<OffsetEdit> = (splices.iter())
        .map(|splice| OffsetEdit {
            start: splice.start,
            old_len: splice.old_len,
            new_len: splice.new_len,
        })
        .collect();
    edits.sort_unstable_by_key(|edit| edit.start);
    Some(follow_offset(offset, &edits, Bias::Left).min(buffer.text().len()))
}

/// The text of `buffer` from `start` to `end`, at most [`WORD_BYTES_MAX`] of it.
fn word_text(buffer: &Buffer, start: usize, end: usize) -> String {
    let text = buffer.text();
    let start = text.clip_offset(start.min(end), Bias::Left);
    let end = text.clip_offset(end.min(start + WORD_BYTES_MAX), Bias::Left);
    let mut word = String::with_capacity(end - start);
    for chunk in text.chunks_in_range(start..end) {
        word.push_str(chunk);
    }
    word
}

/// Where the identifier before `cursor` starts: whole grapheme clusters whose first character
/// is alphanumeric or `_`, within [`WORD_BYTES_MAX`] and the cursor's line.
fn word_start(buffer: &Buffer, cursor: usize) -> usize {
    let text = buffer.text();
    let line = text.line_start(text.offset_to_point(cursor).row);
    let from = line.max(cursor.saturating_sub(WORD_BYTES_MAX));
    let before = word_text(buffer, from, cursor);
    let kept: usize = before
        .graphemes(true)
        .rev()
        .take_while(|cluster| {
            cluster
                .chars()
                .next()
                .is_some_and(|c| c.is_alphanumeric() || c == '_')
        })
        .map(str::len)
        .sum();
    cursor - kept
}

#[cfg(test)]
mod tests;
