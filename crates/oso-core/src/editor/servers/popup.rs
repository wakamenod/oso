//! The completion popup at point (ADR 0138, Completion in the buffer, step 9), as Corfu's: the
//! completions a server offered for the word before the selected window's primary cursor,
//! matched against that word as it is typed, at most [`COMPLETION_POPUP_ROWS`] shown under or over its
//! row, and the selected one inserted at the cursors that share the word.
//!
//! Must not depend on: `command` or `keymap`, or the host's servers, threads or I/O; the host
//! asks the server and hands over the answer, and the popup's keys are the dispatcher's.
//!
//! - The identifier before the cursor is whole grapheme clusters whose first character is
//!   alphanumeric or `_`; it is what is matched, and the popup closes once the cursor is no
//!   longer at its end. What an item replaces starts where the server's first edit does, if
//!   it is on the cursor's line and near it, else at the identifier.
//! - What is matched is an item's `filterText`, else its label, by ADR 0076's matcher, in the
//!   server's order. A row shows the label, then its kind and detail as its annotation, each
//!   cut to what a row can show, its runs of blanks as one, so that no server's text passes as
//!   oso's own.
//! - An item's range is kept only on the cursor's line, around it, and within
//!   [`WORD_BYTES_MAX`] of it: a server's range elsewhere would edit text out of view.
//! - The word and the ranges follow every edit (`Editor::answers_follow`), each offset once;
//!   a reload closes the popup.
//! - Inserting replaces what the item covers at the primary cursor, and the word at each other
//!   cursor whose word before it is the same, with the item's edits besides its own, in one
//!   edit. With no word typed, only the primary's. Those edits, from the answer or the item's
//!   resolve (step 9b), are `extra`'s.
//! - What a closed popup or a dropped answer held is freed on the completion worker.
//! - Nothing of an item reaches a log; `Debug` gives counts.

use std::fmt;
use std::ops::Range;
use std::sync::Arc;

use crate::buffer::{Buffer, BufferId, Replacement};
use crate::complete::{Candidate, Candidates};
use crate::frame::{FloatRegions, at_point};
use crate::selection::{OffsetEdit, follow_sorted};
use crate::sum_tree::Bias;
use crate::window::{Screen, WindowId};

use super::super::Editor;
use super::jumps::AskAt;

/// The most rows of completions the popup shows.
pub const COMPLETION_POPUP_ROWS: usize = 10;

/// The most columns of text the popup shows, and no more than half the window's.
pub const COMPLETION_POPUP_COLUMNS: usize = 60;

/// The longest word before the cursor looked at, and the farthest a server's range reaches
/// from the cursor, in bytes.
const WORD_BYTES_MAX: usize = 256;

/// The most bytes of a label or an annotation a row keeps: more than [`COMPLETION_POPUP_COLUMNS`] show.
const SHOWN_BYTES_MAX: usize = 4 * COMPLETION_POPUP_COLUMNS;

/// A completion as the popup shows and inserts it: its label, its kind and detail as one
/// annotation, what is matched, the text it inserts, and the range of the answer's text it
/// replaces, if the server gave one. `Debug` says nothing of it.
pub struct CompletionPopupItem {
    pub label: Box<str>,
    pub annotation: Box<str>,
    pub filter: Option<Box<str>>,
    pub text: Box<str>,
    pub range: Option<Range<usize>>,
    /// Its edits besides its own, in the same text, such as an added `use` line (step 9b).
    pub extra: Vec<(Range<usize>, Box<str>)>,
}

impl fmt::Debug for CompletionPopupItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompletionPopupItem")
            .finish_non_exhaustive()
    }
}

/// What the host hands over of an answer: its ask's serial, by which an item's resolve names
/// it, where it was asked, the revision of the text its items' ranges are in, its items in the
/// server's order, and whether the server said the list is incomplete, so that typing asks
/// again (step 9b).
#[derive(Debug)]
pub struct CompletionPopupAnswer {
    pub serial: u64,
    pub at: AskAt,
    pub revision: u64,
    pub items: Vec<CompletionPopupItem>,
    pub incomplete: bool,
}

/// What an item inserts, and the range it replaces as places in the offsets followed.
type Insert = (Box<str>, Option<(usize, usize)>);

/// An edit an item makes besides its own: its range as places in the offsets followed, and its
/// text.
type ExtraAt = (usize, usize, Box<str>);

/// The open popup. `Debug` gives counts.
pub struct CompletionPopup {
    window: WindowId,
    buffer: BufferId,
    /// The offsets the popup follows through each edit, ascending and each once: where the
    /// identifier and the replaced text start, and the items' ranges' ends.
    followed: Vec<usize>,
    /// Where the identifier starts and where an item with no range of its own replaces from,
    /// as places in `followed`.
    ident: usize,
    start: usize,
    /// What each item inserts, and its range as places in `followed`.
    inserts: Vec<Insert>,
    /// What is matched, by item.
    matched: Candidates,
    /// What each row shows, by item.
    shown: Arc<[Candidate]>,
    /// The selection was moved, so that `RET` inserts.
    moved: bool,
    /// The answer's ask's serial, and whether the answer said the list is incomplete.
    serial: u64,
    incomplete: bool,
    /// Each item's edits besides its own, by item.
    extras: Vec<Vec<ExtraAt>>,
    /// The item whose resolve was asked for last.
    resolving: Option<usize>,
}

impl fmt::Debug for CompletionPopup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompletionPopup")
            .field("items", &self.inserts.len())
            .field("matches", &self.matched.len())
            .finish_non_exhaustive()
    }
}

impl CompletionPopup {
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

    /// Whether the server said its list is incomplete, so that typing asks again.
    pub fn incomplete(&self) -> bool {
        self.incomplete
    }

    /// The rows shown, at most [`COMPLETION_POPUP_ROWS`] from the first in view: each item's label and
    /// annotation, and whether it is selected.
    pub fn rows(&self) -> impl Iterator<Item = (bool, &Candidate)> {
        let selected = self.matched.selected();
        self.matched
            .shown(COMPLETION_POPUP_ROWS)
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
    pub fn completion_popup(&self) -> Option<&CompletionPopup> {
        self.servers.popup.as_ref()
    }

    /// Opens the popup for `answer`, if nothing moved since it was asked: the selected window
    /// shows its buffer at the revision asked about with the primary cursor where it was, no
    /// prompt is open, and the buffer does not hold secrets. A popup that was open closes.
    ///
    /// # Errors
    ///
    /// `Some("No completions")` with nothing to show, or `None` for a stale answer, which says
    /// nothing.
    pub fn open_completion_popup(
        &mut self,
        answer: CompletionPopupAnswer,
    ) -> Result<(), Option<&'static str>> {
        self.close_completion_popup();
        let (window, buffer) = self.current();
        let cursor = window.selections.primary().cursor;
        let stale = window.buffer() != answer.at.buffer
            || buffer.revision() != answer.at.revision
            || cursor != answer.at.offset
            || self.prompt().is_some()
            || buffer.is_secret();
        if stale || answer.items.is_empty() {
            self.free_on_the_worker(Box::new(answer.items));
            return Err((!stale).then_some("No completions"));
        }
        let mut popup = popup_of(buffer, (window.id(), cursor), answer);
        let text = buffer.text();
        let word = text_of(text, popup.followed[popup.ident]..cursor);
        popup.matched.set_input_with(&word, None);
        if popup.matched.is_empty() {
            self.free_on_the_worker(Box::new(popup));
            return Err(Some("No completions"));
        }
        self.servers.popup = Some(popup);
        Ok(())
    }

    /// Moves the popup's selection `by` rows, from the last to the first and back, as
    /// `corfu-cycle` does.
    pub fn completion_popup_move(&mut self, by: isize) {
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
        popup.matched.keep_in_view(COMPLETION_POPUP_ROWS);
        popup.moved = true;
    }

    /// Closes the popup, if one is open; what it held is freed on the completion worker.
    pub fn close_completion_popup(&mut self) {
        if let Some(popup) = self.servers.popup.take() {
            self.free_on_the_worker(Box::new(popup));
        }
    }

    /// Inserts the popup's selected item and closes it: what the item covers replaced at the
    /// primary cursor, and the word at each other cursor whose word before it is the same, in
    /// one edit.
    ///
    /// # Errors
    ///
    /// The edit's [`crate::buffer::EditError`], or `None` with no popup open or nothing
    /// selected.
    pub fn completion_popup_insert(&mut self) -> Result<(), Option<crate::buffer::EditError>> {
        self.follow_completion_popup();
        let popup = self.servers.popup.take().ok_or(None)?;
        let result = self.insert_selected(&popup);
        self.free_on_the_worker(Box::new(popup));
        result
    }

    fn insert_selected(
        &mut self,
        popup: &CompletionPopup,
    ) -> Result<(), Option<crate::buffer::EditError>> {
        let item = popup.matched.selected_item().ok_or(None)?;
        let (insert, range) = popup.inserts.get(item).ok_or(None)?;
        let (window, buffer) = self.current();
        let text = buffer.text();
        let cursor = window.selections.primary().cursor;
        let at = |place: usize| popup.followed.get(place).copied();
        let (ident, start) = (at(popup.ident).ok_or(None)?, at(popup.start).ok_or(None)?);
        let (from, to) = range
            .and_then(|(from, to)| Some((at(from)?, at(to)?)))
            .map_or((start, cursor), |(from, to)| (from, to.max(cursor)));
        let mut edits = vec![Replacement {
            range: from..to,
            text: insert,
        }];
        let word = text_of(text, ident..cursor);
        let primary = window.selections.primary_index();
        for (index, selection) in window.selections.as_slice().iter().enumerate() {
            let at = selection.cursor;
            if index == primary || word.is_empty() || at < word.len() {
                continue;
            }
            let before = at - word.len();
            if text_is(text, before..at, &word) && word_start(text, at) == before {
                edits.push(Replacement {
                    range: before..at,
                    text: insert,
                });
            }
        }
        // Its edits besides its own, but where one would overlap another (ADR 0138's table).
        let mut extra = Vec::new();
        for (from, to, text) in popup.extras.get(item).into_iter().flatten() {
            let Some(range) = at(*from).zip(at(*to)).map(|(from, to)| from..to) else {
                continue;
            };
            let overlaps = |edit: &Replacement<'_>| {
                range.start < edit.range.end.max(edit.range.start + 1)
                    && edit.range.start < range.end.max(range.start + 1)
            };
            if !edits.iter().any(overlaps) {
                extra.push(range.start);
                edits.push(Replacement { range, text });
            }
        }
        let id = window.buffer();
        let inserted = self.apply_edits(id, &edits).map_err(Some);
        if inserted.is_ok() {
            self.say_extra_out_of_view(id, &extra);
            self.note_inserted(popup, item);
        }
        inserted
    }

    /// After a command: the popup follows the identifier as it is typed, matching it again, and
    /// closes once the cursor is not at its end, another window or buffer shows, a prompt
    /// opened, or nothing matches.
    pub fn follow_completion_popup(&mut self) {
        let Some(popup) = &self.servers.popup else {
            return;
        };
        let (window, buffer) = self.current();
        let cursor = window.selections.primary().cursor;
        let ident = popup.followed.get(popup.ident).copied();
        let kept = window.id() == popup.window
            && window.buffer() == popup.buffer
            && self.prompt().is_none()
            && ident.is_some_and(|ident| {
                ident <= cursor
                    && cursor - ident <= WORD_BYTES_MAX
                    && word_start(buffer.text(), cursor) == ident
            });
        let (true, Some(ident)) = (kept, ident) else {
            self.close_completion_popup();
            return;
        };
        let word = text_of(buffer.text(), ident..cursor);
        let Some(popup) = &mut self.servers.popup else {
            return;
        };
        popup.matched.set_input_with(&word, None);
        if popup.matched.is_empty() {
            self.close_completion_popup();
        }
    }

    /// Where the popup goes over the selected window's text area, as `screen` shows the window
    /// ([`Editor::screen`]): under or over the cursor's row, its text from where the replaced
    /// text starts while that is on the cursor's row, as many rows as match, at most
    /// [`COMPLETION_POPUP_ROWS`], and as wide as the rows shown need, at most [`COMPLETION_POPUP_COLUMNS`] and half
    /// the area.
    pub fn completion_popup_regions(&self, screen: &Screen) -> Option<FloatRegions> {
        let popup = self.completion_popup()?;
        let (window, buffer) = self.current();
        let area = self.tiled_regions().text;
        let display = buffer.display();
        let cell = |offset| {
            let point = display.offset_to_display_point(offset);
            crate::frame::cursor_rect(area, screen, point, 1)
        };
        let cursor = cell(window.selections.primary().cursor)?;
        let start = popup
            .followed
            .get(popup.start)
            .and_then(|start| cell(*start));
        let column = start
            .filter(|start| start.row == cursor.row)
            .map_or(cursor.column, |start| start.column);
        let widest = popup
            .rows()
            .map(|(_, row)| {
                let label = crate::minibuffer::cells_of(row.text());
                let note = crate::minibuffer::cells_of(row.annotation());
                label + if note > 0 { 2 + note } else { 0 }
            })
            .max()
            .unwrap_or(0);
        let columns = widest.clamp(1, COMPLETION_POPUP_COLUMNS.min((area.columns / 2).max(1)));
        let rows = popup.len().min(COMPLETION_POPUP_ROWS);
        at_point(area, (cursor.row, column), (rows, columns))
    }

    /// Moves the popup's word and its items' ranges in `buffer` through an edit's `steps`, as
    /// the cursors move: each offset once a step.
    pub(in crate::editor) fn completion_popup_follows(
        &mut self,
        buffer: BufferId,
        steps: &[Vec<OffsetEdit>],
    ) {
        let Some(popup) = &mut self.servers.popup else {
            return;
        };
        if popup.buffer != buffer {
            return;
        }
        for edits in steps {
            follow_sorted(&mut popup.followed, edits, Bias::Left);
        }
    }

    /// `buffer` was read again: a popup over it no longer knows its word, and closes.
    pub(in crate::editor) fn completion_popup_reloaded(&mut self, buffer: BufferId) {
        if self
            .completion_popup()
            .is_some_and(|popup| popup.buffer == buffer)
        {
            self.close_completion_popup();
        }
    }
}

/// The ranges of `answer`'s items in `buffer`'s text now, each kept only on the line of
/// `cursor`, around it and near it, and their edits besides their own, each while it still
/// maps.
type Spans = Vec<Option<(usize, usize)>>;

fn ranges_now(
    buffer: &Buffer,
    cursor: usize,
    answer: &CompletionPopupAnswer,
) -> (Spans, Vec<Spans>) {
    let text = buffer.text();
    let line = text.line_start(text.offset_to_point(cursor).row);
    // A range of the answer's text in the text now, kept only on the cursor's line, around
    // it and near it.
    let near = |range: &Range<usize>| {
        let start = moved_to(buffer, answer.revision, range.start)?;
        let end = moved_to(buffer, answer.revision, range.end)?;
        let around = line <= start
            && start <= cursor
            && cursor <= end
            && cursor - start <= WORD_BYTES_MAX
            && end - cursor <= WORD_BYTES_MAX
            && !text_holds(text, start..end, '\n');
        around.then_some((start, end))
    };
    let ranges: Spans = (answer.items.iter())
        .map(|item| item.range.as_ref().and_then(near))
        .collect();
    // An item's edits besides its own, in the text now; one that no longer maps goes.
    let extras: Vec<Spans> = (answer.items.iter())
        .map(|item| {
            (item.extra.iter())
                .map(|(range, _)| {
                    let start = moved_to(buffer, answer.revision, range.start)?;
                    let end = moved_to(buffer, answer.revision, range.end)?;
                    Some((start, end.max(start)))
                })
                .collect()
        })
        .collect();
    (ranges, extras)
}

/// The popup over `answer` in `buffer`, shown by the window `window` with its primary cursor at
/// `cursor`: the identifier and the replaced text's start, the items' ranges kept only on the
/// cursor's line, around it and near it, their edits besides their own that still map, and
/// what is matched and shown of each.
fn popup_of(
    buffer: &Buffer,
    (window, cursor): (WindowId, usize),
    answer: CompletionPopupAnswer,
) -> CompletionPopup {
    let buffer_id = answer.at.buffer;
    let text = buffer.text();
    let ident = word_start(text, cursor);
    let (ranges, extras) = ranges_now(buffer, cursor, &answer);
    let start = (ranges.iter().flatten().next())
        .map_or(ident, |(start, _)| *start)
        .min(ident);
    let mut followed: Vec<usize> = [ident, start]
        .into_iter()
        .chain(
            ranges
                .iter()
                .flatten()
                .flat_map(|(start, end)| [*start, *end]),
        )
        .chain(
            extras
                .iter()
                .flatten()
                .flatten()
                .flat_map(|(start, end)| [*start, *end]),
        )
        .collect();
    followed.sort_unstable();
    followed.dedup();
    let place = |offset: usize| followed.binary_search(&offset).unwrap_or(0);
    let mut inserts = Vec::with_capacity(answer.items.len());
    let mut matched = Vec::with_capacity(answer.items.len());
    let mut shown = Vec::with_capacity(answer.items.len());
    let mut extras_at = Vec::with_capacity(answer.items.len());
    for ((item, range), extra) in answer.items.into_iter().zip(ranges).zip(extras) {
        let label = shown_text(&item.label);
        matched.push(Candidate::new(item.filter.unwrap_or(item.label), "", ""));
        shown.push(Candidate::unmatched(label, shown_text(&item.annotation)));
        inserts.push((
            item.text,
            range.map(|(start, end)| (place(start), place(end))),
        ));
        // The edits that still map, each with its text.
        let texts = item.extra.into_iter().map(|(_, text)| text);
        extras_at.push(
            (extra.into_iter().zip(texts))
                .filter_map(|(range, text)| {
                    range.map(|(start, end)| (place(start), place(end), text))
                })
                .collect(),
        );
    }
    let (ident, start) = (place(ident), place(start));
    CompletionPopup {
        window,
        buffer: buffer_id,
        ident,
        start,
        followed,
        inserts,
        matched: Candidates::in_given_order(matched.into()),
        shown: shown.into(),
        moved: false,
        serial: answer.serial,
        incomplete: answer.incomplete,
        extras: extras_at,
        resolving: None,
    }
}

mod extra;
mod text;

use text::{moved_to, shown_text, text_holds, text_is, text_of, word_start};

pub use extra::CompletionTyped;
pub(in crate::editor) use extra::Inserted;

#[cfg(test)]
mod tests;
