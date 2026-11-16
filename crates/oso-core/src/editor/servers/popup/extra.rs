//! An item's edits besides its own (ADR 0138, Completion in the buffer, step 9b): its resolve
//! asked for when it is selected, what the resolve gives taken before the insert or after it,
//! and the lines they change said.
//!
//! Must not depend on: `command` or `keymap`, or the host's servers, threads or I/O; the host
//! asks the server and hands the answer back.
//!
//! - The selected item is resolved once, unless it came with its edits besides its own.
//! - The edits are kept as given, in the text of their revision, and mapped through the edits
//!   since only when the item goes in; when they no longer map, none is made, and the echo
//!   area says so.
//! - An edit that would overlap the item's own, another cursor's or another of its own is
//!   dropped, before the insert and after it alike.
//! - Edits that come once the item was inserted join the insert's undo step afterwards, as
//!   `C-x TAB`'s keys' edits do (ADR 0115), if nothing changed since: the buffer as the insert
//!   left it, and shown. Otherwise none is made, and the echo area says so.
//! - The lines they change out of view are said, in `*Messages*` too, by their numbers only.

use std::ops::Range;

use crate::buffer::{Buffer, BufferId, ContentId, Replacement};
use crate::selection::SelectionSet;

use super::super::super::Editor;
use super::CompletionPopup;
use super::text::moved_to;

/// An item inserted before its resolve came: its answer's serial and place, the buffer, its
/// undo mark before the insert and after it, the cursors before it, and the ranges the insert's
/// edits cover in the text it left.
#[derive(Debug, Clone)]
pub(in crate::editor) struct Inserted {
    answer: u64,
    index: usize,
    buffer: BufferId,
    began: (ContentId, u64),
    after: (ContentId, u64),
    before: SelectionSet,
    covered: Vec<Range<usize>>,
}

/// An item's edits besides its own, as given, with the revision of the text they are in.
type Given = (u64, Vec<(Range<usize>, Box<str>)>);

/// What the echo area says of edits besides an item's own that were not made.
const NOT_MADE: &str = "The completion's other edits no longer fit the text: not made";

impl Editor {
    /// The item the host should resolve, by its answer's serial and its place in the answer:
    /// the selected one, once, unless it came with its edits besides its own.
    pub fn take_completion_resolve(&mut self) -> Option<(u64, usize)> {
        let popup = self.servers.popup.as_mut()?;
        let item = popup.matched.selected_item()?;
        let known = popup.extras.get(item).is_some_and(Option::is_some);
        if known || popup.resolving == Some(item) {
            return None;
        }
        popup.resolving = Some(item);
        Some((popup.serial, item))
    }

    /// Takes the edits besides its own that the item `index` of the answer `answer` makes, in
    /// the text of `revision`: kept with it while the popup is open, or made and joined to the
    /// insert's undo step if it was inserted and nothing changed since. Returns whether the
    /// buffer changed.
    pub fn completion_resolved(
        &mut self,
        (answer, index): (u64, usize),
        revision: u64,
        extra: Vec<(Range<usize>, String)>,
    ) -> bool {
        if extra.is_empty() {
            return false;
        }
        let extra: Vec<(Range<usize>, Box<str>)> = (extra.into_iter())
            .map(|(range, text)| (range, text.into()))
            .collect();
        if let Some(popup) = self.servers.popup.as_mut()
            && popup.serial == answer
        {
            if let Some(slot) = popup.extras.get_mut(index) {
                *slot = Some((revision, extra));
            }
            return false;
        }
        match self.servers.inserted.take() {
            Some(inserted) if (inserted.answer, inserted.index) == (answer, index) => {
                self.apply_late(&inserted, &(revision, extra))
            }
            other => {
                self.servers.inserted = other;
                false
            }
        }
    }

    /// `extra` in the selected window's buffer's text now, but each that would overlap one of
    /// `covered` or another kept before it; `None` when one no longer maps, and the echo area
    /// says so.
    pub(super) fn extras_now(
        &mut self,
        extra: Option<&Given>,
        covered: &[Range<usize>],
    ) -> Option<Vec<(Range<usize>, Box<str>)>> {
        let Some((revision, extra)) = extra else {
            return Some(Vec::new());
        };
        let Some(mapped) = mapped(self.current().1, *revision, extra) else {
            self.message(NOT_MADE);
            return None;
        };
        let mut kept: Vec<(Range<usize>, Box<str>)> = Vec::with_capacity(mapped.len());
        for (range, text) in mapped {
            let clear = !covered.iter().any(|other| overlaps(&range, other))
                && !kept.iter().any(|(other, _)| overlaps(&range, other));
            if clear {
                kept.push((range, text));
            }
        }
        Some(kept)
    }

    /// Makes `extra` after what `inserted` inserted, joined to its undo step, if nothing changed
    /// since; otherwise says they were not made.
    fn apply_late(&mut self, inserted: &Inserted, extra: &Given) -> bool {
        let shown = self.current().0.buffer() == inserted.buffer;
        let unchanged = self
            .buffer(inserted.buffer)
            .is_some_and(|buffer| buffer.undo_mark() == inserted.after);
        if !shown || !unchanged {
            self.message(NOT_MADE);
            return false;
        }
        let Some(kept) = self.extras_now(Some(extra), &inserted.covered) else {
            return false;
        };
        if kept.is_empty() {
            return false;
        }
        let edits: Vec<Replacement<'_>> = (kept.iter())
            .map(|(range, text)| Replacement {
                range: range.clone(),
                text,
            })
            .collect();
        let applied = self.apply_edits(inserted.buffer, &edits).is_ok();
        if applied {
            // One step with the insert's, as the person made it.
            if let Some(buffer) = self.buffer_mut(inserted.buffer) {
                buffer.join_undo_since(inserted.began, Some(inserted.before.clone()));
            }
            self.say_extra_changed(inserted.buffer, &edits, 0);
        }
        applied
    }

    /// The item `item` of `popup` went in with `edits`, from `began` with the cursors `before`:
    /// if its resolve was asked for and has not come, its edits besides its own may still come,
    /// for the insert's undo step.
    pub(super) fn note_inserted(
        &mut self,
        popup: &CompletionPopup,
        item: usize,
        (began, before): (Option<(ContentId, u64)>, SelectionSet),
        edits: &[Replacement<'_>],
    ) {
        let waiting =
            popup.resolving == Some(item) && popup.extras.get(item).is_none_or(Option::is_none);
        let after = self.buffer(popup.buffer).map(Buffer::undo_mark);
        self.servers.inserted = match (waiting, began, after) {
            (true, Some(began), Some(after)) => Some(Inserted {
                answer: popup.serial,
                index: item,
                buffer: popup.buffer,
                began,
                after,
                before,
                covered: covered_after(edits),
            }),
            _ => None,
        };
    }

    /// Says which lines the edits of `edits` past the first `own` changed, as the text is now,
    /// those out of the selected window's view; in `*Messages*` too, by their numbers only.
    pub(super) fn say_extra_changed(
        &mut self,
        buffer: BufferId,
        edits: &[Replacement<'_>],
        own: usize,
    ) {
        if edits.len() <= own || self.current().0.buffer() != buffer {
            return;
        }
        let after = covered_after(edits);
        let (window, shown) = self.current();
        let view = window.shown_bytes(&shown.display());
        let text = shown.text();
        let mut lines: Vec<(usize, usize)> = (after.iter().skip(own))
            .filter(|range| range.start < view.start || range.end > view.end)
            .map(|range| {
                let first = text.offset_to_point(range.start).row + 1;
                // Of its last character: a line it ends with is the one it changed.
                let end = range.end.saturating_sub(1).max(range.start);
                let last = text.offset_to_point(end).row + 1;
                (first, last.max(first))
            })
            .collect();
        if lines.is_empty() {
            return;
        }
        lines.sort_unstable();
        lines.dedup();
        let single = lines.len() == 1 && lines[0].0 == lines[0].1;
        let named: Vec<String> = (lines.iter())
            .map(|&(first, last)| match first == last {
                true => first.to_string(),
                false => format!("{first}–{last}"),
            })
            .collect();
        let noun = if single { "line" } else { "lines" };
        self.message(format!(
            "The completion also changed {noun} {}",
            named.join(", ")
        ));
    }
}

/// `extra` in `buffer`'s text now, from the text of `revision`; `None` once one no longer maps.
fn mapped(
    buffer: &Buffer,
    revision: u64,
    extra: &[(Range<usize>, Box<str>)],
) -> Option<Vec<(Range<usize>, Box<str>)>> {
    (extra.iter())
        .map(|(range, text)| {
            let start = moved_to(buffer, revision, range.start)?;
            let end = moved_to(buffer, revision, range.end)?.max(start);
            Some((start..end, text.clone()))
        })
        .collect()
}

/// Whether two ranges touch: an empty one is taken to cover the place after it, so that two
/// insertions at one place, or one at an edit's start, overlap.
fn overlaps(a: &Range<usize>, b: &Range<usize>) -> bool {
    a.start < b.end.max(b.start + 1) && b.start < a.end.max(a.start + 1)
}

/// The ranges `edits`, of the text before them, cover in the text after them, in their order.
fn covered_after(edits: &[Replacement<'_>]) -> Vec<Range<usize>> {
    let mut order: Vec<usize> = (0..edits.len()).collect();
    order.sort_by_key(|&at| edits[at].range.start);
    let mut shift = 0isize;
    let mut after = vec![0..0; edits.len()];
    for at in order {
        let edit = &edits[at];
        let start = edit.range.start.saturating_add_signed(shift);
        after[at] = start..start + edit.text.len();
        shift += edit.text.len().cast_signed() - edit.range.len().cast_signed();
    }
    after
}

#[cfg(test)]
mod tests;
