//! An undo or a redo staged over several turns (ADR 0119): the group's batches applied a slice
//! at a time on clones of the rope and the display map, and the highlights and the edited lines
//! followed on copies, then committed by moves that cannot panic, as an edit is (ADR 0053). The
//! buffer is not touched until then, so whatever reads it meanwhile sees the state before.
//!
//! Must not depend on: `window`, `editor` or `command`; the editor follows cursors and views.

use std::mem;
use std::ops::Range;

use super::edit::Rewrap;
use super::edited::FollowedLines;
use super::highlighting::FollowedHighlighting;
use super::sliced::{Budget, FOLLOW_WORK, GroupApply, work_of};
use super::{Buffer, EditError};
use crate::display_map::{DisplayMap, DisplaySnapshot};
use crate::rope::Rope;
use crate::selection::{OffsetEdit, SelectionSet};
use crate::syntax::Highlights;
use crate::undo::{Direction, Group};

/// An undo or a redo under way on clones, not yet in the buffer. Dropping it changes nothing.
pub(crate) struct StagedCrossing {
    direction: Direction,
    /// The buffer's revision and its history's count of changes as it began: it goes on only
    /// while both stay.
    revision: u64,
    history: u64,
    text: Rope,
    display: DisplayMap,
    rewrap: Rewrap,
    /// Whether the windows' wraps follow its splices.
    wraps: bool,
    apply: GroupApply,
    /// The group's work, as the slices count it.
    work: usize,
    highlighting: FollowedHighlighting,
    edited: FollowedLines,
    /// Whether all of the group has applied.
    ended: bool,
}

/// What a crossing's commit let go: the group crossed, the groups the history's limit then
/// dropped, the text and display it replaced, and the splices the wraps followed. Dropping it
/// may take milliseconds, so the host drops it off the UI thread (ADR 0119).
#[expect(
    dead_code,
    reason = "it is held only to be dropped where the editor chooses"
)]
pub(crate) struct Freed {
    group: Option<Group>,
    evicted: Vec<Group>,
    highlights: Option<Highlights>,
    text: Rope,
    display: DisplayMap,
    rewrap: Rewrap,
}

impl StagedCrossing {
    /// Which way it goes.
    pub(crate) fn direction(&self) -> Direction {
        self.direction
    }

    /// The share of its work done, from 0 to 1.
    pub(crate) fn done(&self) -> f32 {
        if self.work == 0 {
            return 1.0;
        }
        // Precision does not matter for a share shown in whole percents.
        #[expect(
            clippy::cast_precision_loss,
            reason = "a share shown in whole percents"
        )]
        let share = self.apply.done() as f32 / self.work as f32;
        share.clamp(0.0, 1.0)
    }

    pub(crate) fn display(&self) -> DisplaySnapshot {
        self.display.snapshot()
    }

    /// What the windows' wraps follow, once all of it has applied.
    pub(crate) fn rewrap(&self) -> &Rewrap {
        &self.rewrap
    }

    /// Where cursors may no longer be at a cluster's start, once all of it has applied
    /// (ADR 0091).
    pub(crate) fn changed(&self) -> &[Range<usize>] {
        self.apply.changed()
    }
}

impl Buffer {
    /// Begins the next undo or redo on clones, if the buffer allows one. Nothing applies yet.
    pub(crate) fn stage_crossing(&self, direction: Direction) -> Result<StagedCrossing, EditError> {
        let group = self.next_crossing(direction)?;
        Ok(StagedCrossing {
            direction,
            revision: self.revision,
            history: self.history.changes(),
            text: self.text.clone(),
            display: self.display.clone(),
            rewrap: Rewrap::Splices(Vec::new()),
            wraps: self.wraps,
            apply: GroupApply::default(),
            work: work_of(group.batches().1),
            highlighting: self.follow_highlighting(),
            edited: self.edited.followed(),
            ended: false,
        })
    }

    /// Applies the next slice of `staged`, within `budget`, and follows it on its copies. What
    /// cursors follow goes in `steps`, batch by batch. Returns whether all of the group has
    /// applied.
    ///
    /// # Errors
    ///
    /// [`EditError::Abandoned`] if the buffer or its history changed since it began, or it
    /// began to wrap or stopped; [`EditError::ReadOnly`] if the buffer can no longer be edited.
    /// It is then to be dropped.
    pub(crate) fn apply_slice(
        &self,
        staged: &mut StagedCrossing,
        budget: &mut Budget,
        steps: &mut Vec<Vec<OffsetEdit>>,
    ) -> Result<bool, EditError> {
        if let Some(reason) = self.read_only() {
            return Err(EditError::ReadOnly(reason));
        }
        if !self.crossing_holds(staged) {
            return Err(EditError::Abandoned);
        }
        let group = self
            .history
            .next(staged.direction)
            .ok_or(EditError::Abandoned)?;
        let wraps = match &mut staged.rewrap {
            Rewrap::Splices(splices) if staged.wraps => Some(splices),
            Rewrap::Splices(_) | Rewrap::Whole => None,
        };
        let first = steps.len();
        budget.part += staged.highlighting.composed() * FOLLOW_WORK;
        budget.run = staged.edited.len() * FOLLOW_WORK;
        let (batches, counts) = group.batches();
        let ended = staged.apply.go_on(
            batches,
            counts,
            &mut staged.text,
            &mut staged.display,
            wraps,
            budget,
            steps,
        );
        for step in &steps[first..] {
            staged.highlighting.follow(step);
        }
        // Once a run, as each follow reads every edited range.
        for run in staged.apply.take_runs() {
            staged.edited.follow(&run);
        }
        staged.ended = ended;
        Ok(ended)
    }

    /// Whether the buffer is as `staged` began on it: its revision, its history, its edited
    /// lines but for what it takes in at the commit, and whether it wraps.
    fn crossing_holds(&self, staged: &StagedCrossing) -> bool {
        staged.revision == self.revision
            && staged.history == self.history.changes()
            && !staged.edited.stale(&self.edited)
            && staged.wraps == self.wraps
    }

    /// The cursors of the state a crossing that ended returns to, if its group has them.
    pub(crate) fn crossing_selections(&self, staged: &StagedCrossing) -> Option<&SelectionSet> {
        self.history
            .next(staged.direction)
            .and_then(|group| group.target.selections.as_ref())
    }

    /// Puts a crossing that ended in the buffer, and restores the content id of the state it
    /// returns to. Returns what it let go, for the caller to free. Nothing here can panic part
    /// way. A crossing that has not ended, or that the buffer changed under, changes nothing,
    /// and comes back.
    pub(crate) fn commit_crossing(
        &mut self,
        staged: StagedCrossing,
    ) -> Result<Freed, Box<StagedCrossing>> {
        if !staged.ended || !self.crossing_holds(&staged) {
            return Err(Box::new(staged));
        }
        let Some(group) = self.history.next(staged.direction) else {
            return Err(Box::new(staged));
        };
        let (batches, counts) = staged.apply.into_undo();
        let crossed = group.crossed(batches, counts);
        let highlighting = self.stage_followed(staged.highlighting, staged.text.len());
        let edited = staged.edited.into_lines(&self.edited);

        self.content = crossed.source.content;
        let text = mem::replace(&mut self.text, staged.text);
        let display = mem::replace(&mut self.display, staged.display);
        self.revision += 1;
        self.edited = edited;
        let highlights = self.highlighting.commit(self.revision, highlighting);
        let (group, evicted) = self.history.cross(staged.direction, crossed);
        self.note_crossing();
        Ok(Freed {
            group,
            evicted,
            highlights,
            text,
            display,
            rewrap: staged.rewrap,
        })
    }

    /// Crosses the next group whole, in one slice.
    #[cfg(test)]
    pub(crate) fn cross_whole(&mut self, direction: Direction) -> Result<(), EditError> {
        let mut staged = self.stage_crossing(direction)?;
        let mut budget = Budget::new(usize::MAX, None, super::PIECE);
        let ended = self.apply_slice(&mut staged, &mut budget, &mut Vec::new())?;
        assert!(ended, "a slice with no limit crosses the whole group");
        self.commit_crossing(staged)
            .map_err(|_| EditError::Abandoned)?;
        Ok(())
    }

    /// Each group of the history as its sides and its batches show it, undo groups oldest
    /// first, then redo groups, the next to redo last.
    #[cfg(test)]
    pub(crate) fn history_seen(&self) -> Vec<SeenGroup> {
        let (undo, redo) = self.history.groups();
        undo.iter()
            .chain(redo)
            .map(|group| {
                let batches = super::splice::tests::plain(group.batches().0);
                let side = |side: &crate::undo::Side| (side.content, side.selections.clone());
                (side(&group.target), side(&group.source), batches)
            })
            .collect()
    }
}

/// A group as [`Buffer::history_seen`] shows it.
#[cfg(test)]
pub(crate) type SeenGroup = (
    (super::ContentId, Option<SelectionSet>),
    (super::ContentId, Option<SelectionSet>),
    Vec<Vec<(usize, usize, String)>>,
);
