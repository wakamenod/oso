//! How the cursors and marks of every window follow a change to a buffer they show,
//! and are put where a cursor can be afterwards (ADR 0053, ADR 0071).
//!
//! Must not depend on: `command` or `keymap`; it works out sets of selections from
//! offsets and a display snapshot, and the editor swaps them in.

use std::ops::Range;

use super::Editor;
use crate::buffer::BufferId;
use crate::display_map::DisplaySnapshot;
use crate::motion;
use crate::selection::{MarkActive, OffsetEdit, SelectionSet, new_text_ends};
use crate::sum_tree::Bias;

/// What the selected window's cursors do when a buffer it shows changes.
#[derive(Clone, Copy)]
pub(super) enum Selected<'a> {
    /// It made the edit: a cursor after new text stays after it.
    Edited,
    /// An undo or a redo returns it to these cursors, if the group has them.
    Restored(Option<&'a SelectionSet>),
    /// A message went to `*Messages*`: as an edit, but what a person set there stays.
    Logged,
}

/// Moves every cursor where a cursor can be: to the start of the display cluster
/// around it, or to the end if it is at one of `ends` (ascending), just after new
/// text. New text can join what follows it, and what was typed must stay before
/// the cursor. Marks, and those in the ring, go to the start of theirs. A cursor
/// loses its goal if it moved, or with `clear_goals` always, as after an edit.
///
/// With `near`, ranges ascending and apart, only what is in one of them, or at its
/// end, is clipped: an edit says where clusters may have changed, and anything
/// elsewhere is where a cursor can be already (ADR 0091).
pub(super) fn clip_selections(
    set: &mut SelectionSet,
    display: &DisplaySnapshot,
    ends: &[usize],
    clear_goals: bool,
    near: Option<&[Range<usize>]>,
) {
    let clips = |offset: usize| {
        near.is_none_or(|ranges| {
            let index = ranges.partition_point(|range| range.end < offset);
            ranges.get(index).is_some_and(|range| range.start <= offset)
        })
    };
    let start = |offset| {
        if clips(offset) {
            motion::cluster_start(display, offset)
        } else {
            offset
        }
    };
    let mut ends = ends.iter().copied().peekable();
    set.update(|selection| {
        while ends.next_if(|end| *end < selection.cursor).is_some() {}
        let cursor = if ends.peek() == Some(&selection.cursor) {
            motion::cluster_end(display, selection.cursor)
        } else {
            start(selection.cursor)
        };
        if clear_goals || cursor != selection.cursor {
            selection.goal = None;
        }
        selection.cursor = cursor;
        selection.mark = selection.mark.map(start);
        Ok::<(), ()>(())
    });
    set.update_ring(start);
}

impl Editor {
    /// The cursors of every window showing `id` once a change is in, by window
    /// position. They follow `steps`, as markers except in the selected window,
    /// which does as `selected` says, but for its ring of marks, which follows.
    /// Each ends where a cursor can be. A change hides every region of the buffer
    /// (ADR 0071), but a message logged to `*Messages*`.
    pub(super) fn moved_selections(
        &self,
        id: BufferId,
        display: &DisplaySnapshot,
        steps: &[Vec<OffsetEdit>],
        changed: &[Range<usize>],
        selected: Selected<'_>,
    ) -> Vec<(usize, SelectionSet)> {
        let mut moved = Vec::new();
        for (index, window) in self.windows.iter().enumerate() {
            if window.buffer() != id {
                continue;
            }
            if let Selected::Restored(Some(restored)) = selected
                && index == self.selected
            {
                let mut ring = *window.selections.mark_ring();
                steps.iter().for_each(|edits| ring.follow_edits(edits));
                let mut set = restored.with_ring(ring);
                set.set_mark_active(MarkActive::Inactive);
                clip_selections(&mut set, display, &[], true, Some(changed));
                moved.push((index, set));
                continue;
            }
            let mut set = window.selections.clone();
            let logged = matches!(selected, Selected::Logged);
            if !logged {
                set.set_mark_active(MarkActive::Inactive);
            }
            let edited =
                index == self.selected && matches!(selected, Selected::Edited | Selected::Logged);
            let bias = if edited { Bias::Right } else { Bias::Left };
            let mut ends = Vec::new();
            for edits in steps {
                if logged {
                    // What a person set there stays as it was: an empty region too.
                    set.follow_logged(edits, bias);
                } else {
                    set.follow_edits(edits, bias);
                }
                if edited {
                    ends.extend(new_text_ends(edits));
                }
            }
            clip_selections(&mut set, display, &ends, true, Some(changed));
            moved.push((index, set));
        }
        moved
    }
}
