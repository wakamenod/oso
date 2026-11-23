//! How the cursors and marks of every window follow a change to a buffer they show,
//! and are put where a cursor can be afterwards (ADR 0053, ADR 0071), how isearch and
//! a jump are kept true to the text they show before the host waits (ADR 0082, ADR 0116), and
//! how the other places the editor marks in a buffer follow each edit: servers' answers
//! (ADR 0138) and dictation's spans (ADR 0175).
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

impl Editor {
    /// The places marked in `buffer` follow an edit's `steps`, as the cursors do: servers'
    /// answers and dictation's spans.
    pub(super) fn marks_follow(&mut self, buffer: BufferId, steps: &[Vec<OffsetEdit>]) {
        self.answers_follow(buffer, steps);
        self.dictation_spans_follow(buffer, steps);
    }

    /// `buffer` was read again, its offsets moved through `map`: servers' answers move or go,
    /// and dictation's spans, whose text is no longer known, go.
    pub(super) fn marks_reloaded(&mut self, buffer: BufferId, map: impl FnMut(usize) -> usize) {
        self.answers_reloaded(buffer, map);
        self.forget_dictation_spans(buffer);
    }

    /// The places marked in `buffer` follow what a sliced undo's step did since `before`, as
    /// the batch the buffer keeps says (ADR 0119). A buffer that keeps none leaves servers'
    /// answers where they were. Dictation's spans follow the step's own edits, `span_steps`,
    /// when they were kept, else that batch, and go if neither is there, as where they are is
    /// not known.
    pub(super) fn marks_cross(
        &mut self,
        buffer: BufferId,
        before: u64,
        span_steps: Option<&[Vec<OffsetEdit>]>,
    ) {
        let batch = self.buffer(buffer).and_then(|shown| {
            let splices = shown.edits_since(before)?;
            let mut edits: Vec<OffsetEdit> = splices
                .iter()
                .map(|splice| OffsetEdit {
                    start: splice.start,
                    old_len: splice.old_len,
                    new_len: splice.new_len,
                })
                .collect();
            edits.sort_unstable_by_key(|edit| edit.start);
            Some(edits)
        });
        if let Some(edits) = &batch {
            self.answers_follow(buffer, std::slice::from_ref(edits));
        }
        match (span_steps, &batch) {
            (Some(steps), _) => self.dictation_spans_follow(buffer, steps),
            (None, Some(edits)) => self.dictation_spans_follow(buffer, std::slice::from_ref(edits)),
            (None, None) => self.forget_dictation_spans(buffer),
        }
    }

    /// Keeps isearch and a jump true to their windows and their text: isearch ends if its
    /// window no longer shows its buffer, or a prompt is open, and searches again if the text
    /// changed; a jump reading its string finds its matches again, and one showing labels ends,
    /// saying so. Returns whether what shows changed. The host calls this before it waits.
    pub fn follow_text(&mut self) -> bool {
        let jumped = self.jump_follow_text();
        self.isearch_follow_text() || jumped
    }
}

/// What the cursors that do not follow a change as markers do: the selected window's, or a
/// transcript's at its end.
#[derive(Clone, Copy)]
pub(super) enum Selected {
    /// It made the edit: a cursor after new text stays after it.
    Edited,
    /// A message went to `*Messages*`: as an edit, but what a person set there stays.
    Logged,
    /// A native session's rows changed (ADR 0165): in every window, selected or not, the cursor
    /// at the end of the text stays there, and any other follows as a marker. What a person set
    /// there stays.
    Tail,
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
    // Every test that clips near edits checks that clipping everything gives the same.
    #[cfg(test)]
    let full = near.map(|_| {
        let mut full = set.clone();
        clip_selections(&mut full, display, ends, clear_goals, None);
        full
    });
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
    #[cfg(test)]
    if let Some(full) = full {
        assert_eq!(
            *set, full,
            "clipping near {near:?} differs from clipping all"
        );
    }
}

impl Editor {
    /// The cursors of every window showing `id` once a change is in, by window
    /// position. They follow `steps`, as markers except as `selected` says. Each ends where a
    /// cursor can be. A change hides every region of the buffer (ADR 0071), but a message logged
    /// to `*Messages*` and a native session's rows.
    pub(super) fn moved_selections(
        &self,
        id: BufferId,
        display: &DisplaySnapshot,
        steps: &[Vec<OffsetEdit>],
        changed: &[Range<usize>],
        selected: Selected,
    ) -> Vec<(usize, SelectionSet)> {
        let mut moved = Vec::new();
        for (index, window) in self.windows.iter().enumerate() {
            if window.buffer() != id {
                continue;
            }
            let mut set = window.selections.clone();
            let logged = matches!(selected, Selected::Logged | Selected::Tail);
            if !logged {
                set.set_mark_active(MarkActive::Inactive);
            }
            let edited = match selected {
                Selected::Edited | Selected::Logged => index == self.selected,
                Selected::Tail => self.at_native_end(index),
            };
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
