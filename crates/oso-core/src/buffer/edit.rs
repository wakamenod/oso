//! Editing a buffer in two steps (ADR 0053): an edit, an undo or a redo is made on clones of
//! the rope and the display map, both O(1) to clone, and then committed by moves that cannot
//! panic, with its undo record.
//!
//! Must not depend on: `window`, `editor` or `command`; the editor maps cursors between the two
//! steps.

use std::ops::Range;
use std::slice;

use super::splice::{SpliceText, apply_group, apply_marking, normalized, offset_edit};
use super::{Buffer, EditError, Replacement};
use crate::display_map::wrap_map::Splice as WrapSplice;
use crate::display_map::{DisplayMap, DisplaySnapshot};
use crate::rope::Rope;
use crate::selection::{OffsetEdit, SelectionSet};
#[cfg(test)]
use crate::undo::History;
use crate::undo::{Batch, ContentId, Direction, Group, Opening, Side, Splice};

/// What the windows' wraps follow of a change (ADR 0100, ADR 0102).
pub(crate) enum Rewrap {
    /// Its splices, each with the display before and after it, in the order they applied;
    /// none while the buffer does not wrap.
    Splices(Vec<WrapSplice>),
    /// A whole new text with a display map of its own, from which no row carries over.
    Whole,
}

/// An edit made on clones, not yet in the buffer. Dropping it changes nothing.
pub(crate) struct StagedEdit {
    pub(super) text: Rope,
    pub(super) display: DisplayMap,
    pub(super) edits: Vec<OffsetEdit>,
    pub(super) undo: Batch,
    pub(super) changed: Vec<Range<usize>>,
    pub(super) rewrap: Rewrap,
}

impl StagedEdit {
    /// What the windows' wraps follow.
    pub(crate) fn rewrap(&self) -> &Rewrap {
        &self.rewrap
    }

    pub(crate) fn display(&self) -> DisplaySnapshot {
        self.display.snapshot()
    }

    /// What cursors follow, batch by batch.
    pub(crate) fn steps(&self) -> &[Vec<OffsetEdit>] {
        slice::from_ref(&self.edits)
    }

    /// Where cursors may no longer be at a cluster's start, afterwards (ADR 0091).
    pub(crate) fn changed(&self) -> &[Range<usize>] {
        &self.changed
    }
}

/// An undo or a redo made on clones, not yet in the buffer.
pub(crate) struct StagedCrossing {
    text: Rope,
    display: DisplayMap,
    rewrap: Rewrap,
    steps: Vec<Vec<OffsetEdit>>,
    direction: Direction,
    crossed: Group,
    changed: Vec<Range<usize>>,
}

impl StagedCrossing {
    pub(crate) fn display(&self) -> DisplaySnapshot {
        self.display.snapshot()
    }

    /// What the windows' wraps follow.
    pub(crate) fn rewrap(&self) -> &Rewrap {
        &self.rewrap
    }

    /// What cursors follow, batch by batch.
    pub(crate) fn steps(&self) -> &[Vec<OffsetEdit>] {
        &self.steps
    }

    /// Where cursors may no longer be at a cluster's start, afterwards (ADR 0091).
    pub(crate) fn changed(&self) -> &[Range<usize>] {
        &self.changed
    }

    /// The cursors of the state this returns to, if the group has them.
    pub(crate) fn selections(&self) -> Option<&SelectionSet> {
        self.crossed.source.selections.as_ref()
    }
}

impl Buffer {
    pub(super) fn new_content(&mut self) -> ContentId {
        let id = ContentId(self.next_content);
        self.next_content += 1;
        id
    }

    /// Makes `edits` on clones, unless the buffer is read-only. `None` if all
    /// are empty.
    pub(crate) fn stage_edits(
        &self,
        edits: &[Replacement<'_>],
    ) -> Result<Option<StagedEdit>, EditError> {
        if let Some(reason) = self.read_only() {
            return Err(EditError::ReadOnly(reason));
        }
        let splices = normalized(&self.text, edits);
        Ok((!splices.is_empty()).then(|| self.stage_splices(&splices)))
    }

    pub(super) fn stage_splices<T: SpliceText>(&self, splices: &[Splice<T>]) -> StagedEdit {
        let mut text = self.text.clone();
        let mut display = self.display.clone();
        let mut changed = Vec::new();
        let mut wraps = Vec::new();
        let collect = self.wraps.then_some(&mut wraps);
        let undo = apply_marking(splices, &mut text, &mut display, &mut changed, collect);
        StagedEdit {
            text,
            display,
            edits: splices.iter().map(offset_edit).collect(),
            undo,
            changed,
            rewrap: Rewrap::Splices(wraps),
        }
    }

    /// Puts a staged edit in the buffer, with its undo record and the cursors
    /// `after` it. Nothing here can panic part way.
    pub(crate) fn commit_edit(
        &mut self,
        staged: StagedEdit,
        opening: Opening,
        after: Option<SelectionSet>,
    ) {
        let before = self.content;
        self.content = self.new_content();
        self.text = staged.text;
        self.display = staged.display;
        self.revision += 1;
        if self.no_history {
            return;
        }
        let after = Side {
            content: self.content,
            selections: after,
        };
        self.history.record(staged.undo, opening, before, after);
    }

    /// The group the next undo or redo crosses, if the buffer allows one.
    pub(crate) fn next_crossing(&self, direction: Direction) -> Result<&Group, EditError> {
        if let Some(reason) = self.read_only() {
            return Err(EditError::ReadOnly(reason));
        }
        self.history.next(direction).ok_or(match direction {
            Direction::Undo => EditError::NoUndo,
            Direction::Redo => EditError::NoRedo,
        })
    }

    /// Makes the next undo or redo on clones, if there is one.
    pub(crate) fn stage_crossing(&self, direction: Direction) -> Result<StagedCrossing, EditError> {
        let group = self.next_crossing(direction)?;
        let mut text = self.text.clone();
        let mut display = self.display.clone();
        let mut wraps = Vec::new();
        let collect = self.wraps.then_some(&mut wraps);
        let (steps, batches, changed) =
            apply_group(&group.batches, &mut text, &mut display, collect);
        Ok(StagedCrossing {
            text,
            display,
            rewrap: Rewrap::Splices(wraps),
            steps,
            direction,
            crossed: group.crossed(batches),
            changed,
        })
    }

    /// Puts a staged undo or redo in the buffer, and restores the content id
    /// of the state it returns to.
    pub(crate) fn commit_crossing(&mut self, staged: StagedCrossing) {
        self.content = staged.crossed.source.content;
        self.text = staged.text;
        self.display = staged.display;
        self.revision += 1;
        self.history.cross(staged.direction, staged.crossed);
        self.note_crossing();
    }

    /// Whether a command that starts with cursors `selections` may join the
    /// newest undo group.
    pub(crate) fn can_join_undo(&self, selections: Option<&SelectionSet>) -> bool {
        self.history.can_join(self.content, selections)
    }

    /// Sets the cursors the newest undo group ends with.
    pub(crate) fn set_undo_after(&mut self, selections: SelectionSet) {
        self.history.set_after(selections);
    }

    #[cfg(test)]
    pub(crate) fn set_undo_limit(&mut self, limit: usize) {
        self.history = History::new(limit);
    }

    #[cfg(test)]
    pub(crate) fn undo_lens(&self) -> (usize, usize) {
        self.history.lens()
    }
}
