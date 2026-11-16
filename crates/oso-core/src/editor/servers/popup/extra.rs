//! What the completion popup asks of the host and takes back after it opened (ADR 0138,
//! Completion in the buffer, step 9b): an item's resolve when it is selected, its edits besides
//! its own once resolved, before the insert or after it, and what the person typed, which may
//! open the popup by itself or ask again for an incomplete list.
//!
//! Must not depend on: `command` or `keymap`, or the host's servers, threads or I/O; the host
//! asks the server and hands the answer back.
//!
//! - The selected item is resolved once, unless it came with its edits besides its own.
//! - Edits that come once the item was inserted join the insert's undo step if nothing changed
//!   since: the buffer still at the revision the insert left, the cursors where it left them.
//!   Otherwise they are dropped, and the echo area says so.
//! - Where an edit besides the item's own changes a line out of view, the echo area says which.
//! - Only the person's own typing is noted (the dispatcher's), never the agent's, a replayed
//!   macro's or a yank.

use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;

use crate::buffer::{BufferId, Replacement};
use crate::window::Screen;

use super::super::super::{Editor, UndoScope};
use super::super::jumps::AskAt;
use super::CompletionPopup;
use super::text::{moved_to, text_of, word_start};

/// An item inserted before its resolve came: its answer's serial and place, the buffer, and
/// the revision the insert left it at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::editor) struct Inserted {
    answer: u64,
    index: usize,
    buffer: BufferId,
    revision: u64,
}

/// What the person typed into a buffer, for the host to judge whether completions are asked
/// for: where, the character before the cursor, and how many grapheme clusters the identifier
/// before it has, at most what a word is looked at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompletionTyped {
    pub at: AskAt,
    pub last: char,
    pub word: usize,
}

impl Editor {
    /// The item the host should resolve, by its answer's serial and its place in the answer:
    /// the selected one, once, unless it came with its edits besides its own.
    pub fn take_completion_resolve(&mut self) -> Option<(u64, usize)> {
        let popup = self.servers.popup.as_mut()?;
        let item = popup.matched.selected_item()?;
        let known = popup
            .extras
            .get(item)
            .is_some_and(|extras| !extras.is_empty());
        if known || popup.resolving == Some(item) {
            return None;
        }
        popup.resolving = Some(item);
        Some((popup.serial, item))
    }

    /// Takes the edits besides its own that the item `index` of the answer `answer` makes, in
    /// the text of `revision`: kept with it while the popup is open, or applied in the insert's
    /// undo step if it was inserted and nothing changed since. Returns whether the buffer
    /// changed.
    pub fn completion_resolved(
        &mut self,
        (answer, index): (u64, usize),
        revision: u64,
        extra: Vec<(Range<usize>, String)>,
    ) -> bool {
        if extra.is_empty() {
            return false;
        }
        if self
            .servers
            .popup
            .as_ref()
            .is_some_and(|popup| popup.serial == answer)
        {
            self.keep_resolved(index, revision, extra);
            return false;
        }
        match self.servers.inserted.take() {
            Some(inserted) if (inserted.answer, inserted.index) == (answer, index) => {
                self.apply_late(inserted, revision, &extra)
            }
            other => {
                self.servers.inserted = other;
                false
            }
        }
    }

    /// Keeps `extra` with the open popup's item `index`, followed through edits from now on.
    fn keep_resolved(&mut self, index: usize, revision: u64, extra: Vec<(Range<usize>, String)>) {
        let Some(buffer) = self
            .servers
            .popup
            .as_ref()
            .and_then(|popup| self.buffer(popup.buffer))
        else {
            return;
        };
        let mapped: Vec<(usize, usize, Box<str>)> = (extra.into_iter())
            .filter_map(|(range, text)| {
                let start = moved_to(buffer, revision, range.start)?;
                let end = moved_to(buffer, revision, range.end)?.max(start);
                Some((start, end, text.into()))
            })
            .collect();
        let Some(popup) = self.servers.popup.as_mut() else {
            return;
        };
        if index >= popup.extras.len() {
            return;
        }
        let mapped = refollow(mapped, popup);
        popup.extras[index] = mapped;
    }

    /// Applies `extra`, in the text of `revision`, to what `inserted` inserted, joining its
    /// undo step, if nothing changed since; otherwise says they were dropped.
    fn apply_late(
        &mut self,
        inserted: Inserted,
        revision: u64,
        extra: &[(Range<usize>, String)],
    ) -> bool {
        let shown = self.current().0.buffer() == inserted.buffer;
        let Some(buffer) = self.buffer(inserted.buffer).filter(|_| shown) else {
            return false;
        };
        let unchanged = buffer.revision() == inserted.revision;
        let edits: Option<Vec<Replacement<'_>>> = (extra.iter())
            .map(|(range, text)| {
                let start = moved_to(buffer, revision, range.start)?;
                let end = moved_to(buffer, revision, range.end)?.max(start);
                Some(Replacement {
                    range: start..end,
                    text,
                })
            })
            .collect();
        let selections = self.windows[self.selected].selections.clone();
        let joins = unchanged && buffer.can_join_undo(Some(&selections));
        let Some(edits) = edits.filter(|_| joins) else {
            self.message("The completion's other edits came after the text changed: not made");
            return false;
        };
        let starts: Vec<usize> = edits.iter().map(|edit| edit.range.start).collect();
        // A step of the host's own, joined to the insert's, as a command's that allows it is.
        self.undo_scope = Some(UndoScope {
            before: (inserted.buffer, selections),
            touched: Vec::new(),
            join: true,
            apart: false,
            crossed: false,
        });
        self.join_ready = true;
        let applied = self.apply_edits(inserted.buffer, &edits).is_ok();
        self.end_undo_group();
        if applied {
            self.say_extra_out_of_view(inserted.buffer, &starts);
        }
        applied
    }

    /// The item `item` of `popup` was inserted: if its resolve was asked for and has not come,
    /// its edits besides its own may still come, for the insert's undo step.
    pub(super) fn note_inserted(&mut self, popup: &CompletionPopup, item: usize) {
        let waiting =
            popup.resolving == Some(item) && popup.extras.get(item).is_none_or(Vec::is_empty);
        self.servers.inserted = waiting
            .then(|| {
                let buffer = self.buffer(popup.buffer)?;
                Some(Inserted {
                    answer: popup.serial,
                    index: item,
                    buffer: popup.buffer,
                    revision: buffer.revision(),
                })
            })
            .flatten();
    }

    /// Says which line an edit at one of `starts` in `buffer` changed, the first out of the
    /// selected window's view, if any is.
    pub(super) fn say_extra_out_of_view(&mut self, buffer: BufferId, starts: &[usize]) {
        if starts.is_empty() || self.current().0.buffer() != buffer {
            return;
        }
        let mut screen = Screen::default();
        self.screen(&mut screen);
        let area = self.tiled_regions().text;
        let shown = self.current().1;
        let display = shown.display();
        let out = starts.iter().find(|start| {
            let point = display.offset_to_display_point(**start);
            crate::frame::cursor_rect(area, &screen, point, 1).is_none()
        });
        if let Some(start) = out {
            let line = shown.text().offset_to_point(*start).row + 1;
            self.echo(format!("The completion also changed line {line}"));
        }
    }

    /// The person typed into the selected window's buffer: noted for the host
    /// ([`Editor::take_completion_typed`]), unless the buffer may hold secrets or a prompt is
    /// open. The dispatcher calls it after the person's `self-insert-command` only.
    pub fn note_completion_typed(&mut self) {
        let (window, buffer) = self.current();
        if self.prompt().is_some() || buffer.is_secret() {
            return;
        }
        let cursor = window.selections.primary().cursor;
        let text = buffer.text();
        let start = word_start(text, cursor);
        let before = text_of(text, start..cursor);
        let last = text_of(text, cursor.saturating_sub(4)..cursor)
            .chars()
            .next_back();
        let Some(last) = last else {
            return;
        };
        self.servers.typed = Some(CompletionTyped {
            at: self.ask_at(),
            last,
            word: before.graphemes(true).count(),
        });
    }

    /// What the person last typed, once.
    pub fn take_completion_typed(&mut self) -> Option<CompletionTyped> {
        self.servers.typed.take()
    }
}

/// `added`'s offsets put among those `popup` follows, and every place in `popup` moved to where
/// its offset is now among them; `added` comes back as places.
fn refollow(
    added: Vec<(usize, usize, Box<str>)>,
    popup: &mut CompletionPopup,
) -> Vec<(usize, usize, Box<str>)> {
    let followed = std::mem::take(&mut popup.followed);
    let mut all = followed.clone();
    all.extend(added.iter().flat_map(|(start, end, _)| [*start, *end]));
    all.sort_unstable();
    all.dedup();
    let place = |old: usize| {
        let offset = followed.get(old).copied().unwrap_or(0);
        all.binary_search(&offset).unwrap_or(0)
    };
    popup.ident = place(popup.ident);
    popup.start = place(popup.start);
    for (_, range) in &mut popup.inserts {
        if let Some((from, to)) = range {
            *range = Some((place(*from), place(*to)));
        }
    }
    for extras in &mut popup.extras {
        for (from, to, _) in extras {
            (*from, *to) = (place(*from), place(*to));
        }
    }
    let at = |offset: usize| all.binary_search(&offset).unwrap_or(0);
    let added = (added.into_iter())
        .map(|(start, end, text)| (at(start), at(end), text))
        .collect();
    popup.followed = all;
    added
}

#[cfg(test)]
mod tests;
