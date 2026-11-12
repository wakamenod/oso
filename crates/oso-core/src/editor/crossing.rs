//! Undo and redo a slice at a time (ADR 0119): the editor keeps the crossing under way, runs a
//! slice of it at each turn of the host's loop, follows every window's cursors and top on
//! copies slice by slice, and commits each step as it ends, so that the buffer is always at a
//! state of its history. A count crosses step after step. The echo line says how far it is.
//!
//! Must not depend on: `command` or `keymap`, or the host's threads; the host asks for each
//! slice.

use std::fmt::{self, Write as _};
use std::time::{Duration, Instant};

use super::Editor;
use super::follow::clip_selections;
use super::wraps::MovedViews;
use crate::buffer::{Budget, BufferId, EditError, FOLLOW_WORK, PIECE, StagedCrossing};
use crate::selection::{MarkActive, SelectionSet, follow_offset};
use crate::sum_tree::Bias;
use crate::undo::Direction;
use crate::window::WindowId;

/// The most time a turn gives work that goes on over turns: a slice of `!`, which a pattern
/// can make read far past each match (ADR 0113), or of an undo (ADR 0119).
pub(super) const SLICE_TIME: Duration = Duration::from_millis(3);

/// The least time a slice applies for, however long what follows the text took last time.
const APPLY_TIME: Duration = Duration::from_millis(1);

/// The most work a slice does, as [`Budget`] counts it: about what 3 ms does, so that the time
/// stops a slice first, and a slow machine still shows progress.
const SLICE_WORK: usize = 4 * 1024 * 1024;

/// The work a slice does before its time may stop it: 256 splices, a fraction of a
/// millisecond, so that a small undo crosses in its command however slow the machine, as ADR
/// 0119 has small undos unchanged.
const SURE_WORK: usize = 256 * crate::buffer::SPLICE_WORK;

/// What the editor keeps of undo and redo a slice at a time.
#[derive(Default)]
pub(super) struct Crossings {
    /// The crossing under way.
    under_way: Option<Crossing>,
    /// What each commit let go, and each crossing dropped, until the host takes it to free
    /// off the UI thread (ADR 0119).
    freed: Vec<Box<dyn Send>>,
    /// What each slice does in tests: its work and the bytes of a piece.
    #[cfg(test)]
    slices: Option<(usize, usize)>,
    /// In tests, each slice's time is up as it begins, as on a machine far slower than any.
    #[cfg(test)]
    no_time: bool,
}

/// An undo or a redo under way in the selected window's buffer, a slice at a time
/// (ADR 0119).
pub struct Crossing {
    buffer: BufferId,
    /// The window selected as it began, which gets the cursors the step returns to.
    window: WindowId,
    staged: StagedCrossing,
    /// Each window showing the buffer, by position.
    windows: Vec<Followed>,
    /// Steps of the count still to cross after this one.
    left: usize,
    /// Slices run so far, of every step of the count.
    slices: usize,
    /// How long the last slice went on past the time it applied for, following what it
    /// applied, as highlights do: the next applies for that much less.
    over: Duration,
}

/// A window's cursors and top following a crossing on copies.
struct Followed {
    id: WindowId,
    /// Its cursors as the step began, which nothing else may change meanwhile.
    before: SelectionSet,
    set: SelectionSet,
    top: usize,
}

impl Crossing {
    /// Whether it redoes; otherwise it undoes.
    pub fn redo(&self) -> bool {
        self.staged.direction() == Direction::Redo
    }

    /// The share of the current step's work done, from 0 to 1. It never goes back while the
    /// step goes on.
    pub fn done(&self) -> f32 {
        self.staged.done()
    }

    /// The steps of the count still to cross after this one.
    pub fn left(&self) -> usize {
        self.left
    }
}

impl fmt::Debug for Crossing {
    /// Leaves out the text and the offsets.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Crossing")
            .field("redo", &self.redo())
            .field("done", &self.done())
            .field("left", &self.left)
            .finish_non_exhaustive()
    }
}

impl Editor {
    /// Starts crossing `count` steps (undo if positive, redo if negative) in the selected
    /// window's buffer and runs the first slice. A crossing already under way is dropped once
    /// the first step can begin. With a count of 0, nothing happens.
    ///
    /// # Errors
    ///
    /// The first step's [`EditError::NoUndo`], [`EditError::NoRedo`] or
    /// [`EditError::ReadOnly`], and nothing changes then. An error at a later step met in the
    /// first slice comes back here too, after the steps before it.
    pub fn begin_crossing(&mut self, count: isize) -> Result<(), EditError> {
        let direction = match count.signum() {
            0 => return Ok(()),
            1 => Direction::Undo,
            _ => Direction::Redo,
        };
        let id = self.windows[self.selected].buffer();
        let buffer = self.buffer(id).ok_or(EditError::NoSuchBuffer)?;
        buffer.next_crossing(direction)?;
        self.cancel_crossing();
        // Edits of this command come first in the history, as their own step.
        if let Some(scope) = &mut self.undo_scope
            && let Some(position) = scope.touched.iter().position(|other| *other == id)
        {
            scope.touched.swap_remove(position);
            self.seal_undo_group(id);
        }
        self.crossing.under_way = Some(self.next_step(id, direction, count.unsigned_abs() - 1)?);
        self.go_on_crossing().unwrap_or(Ok(()))
    }

    /// The crossing under way, if any.
    pub fn crossing(&self) -> Option<&Crossing> {
        self.crossing.under_way.as_ref()
    }

    /// The next slice, for the host at each turn: `None` while it goes on, `Some(result)` when
    /// it ended, and `Some(Ok(()))` with none under way. Each step commits as it ends. Outside
    /// a command, the host gives the wraps their budget first ([`Editor::refill_wraps`]) and
    /// makes the windows follow their cursors after ([`Editor::follow_cursor`]), as the
    /// dispatcher does around a command, and frees what [`Editor::take_crossing_freed`] gives.
    ///
    /// # Errors
    ///
    /// Inside `Some`: a later step's [`EditError::NoUndo`] or [`EditError::NoRedo`], which ends
    /// it after the steps before; [`EditError::Abandoned`] if the buffer, its history, its
    /// windows or their cursors changed under the step, which then changes nothing, as a panic
    /// in a slice does; [`EditError::ReadOnly`] if the buffer can no longer be edited.
    pub fn go_on_crossing(&mut self) -> Option<Result<(), EditError>> {
        // Taken out while the slice runs, so that a panic drops it and nothing else.
        let Some(mut crossing) = self.crossing.under_way.take() else {
            return Some(Ok(()));
        };
        let started = Instant::now();
        let applying = SLICE_TIME.saturating_sub(crossing.over).max(APPLY_TIME);
        let mut budget = self.crossing_budget(started + applying);
        loop {
            match self.crossing_slice(&mut crossing, &mut budget) {
                Ok(true) => {}
                Ok(false) => {
                    crossing.slices += 1;
                    crossing.over = started.elapsed().saturating_sub(applying);
                    self.crossing.under_way = Some(crossing);
                    return None;
                }
                Err(error) => {
                    self.crossing.freed.push(Box::new(crossing));
                    return Some(Err(error));
                }
            }
            let (id, direction, left, slices) = (
                crossing.buffer,
                crossing.staged.direction(),
                crossing.left,
                crossing.slices,
            );
            if let Err(error) = self.commit_step(crossing) {
                return Some(Err(error));
            }
            if left == 0 {
                return Some(Ok(()));
            }
            crossing = match self.next_step(id, direction, left - 1) {
                Ok(next) => next,
                Err(error) => return Some(Err(error)),
            };
            crossing.slices = slices;
            if budget.spent() {
                crossing.slices += 1;
                self.crossing.under_way = Some(crossing);
                return None;
            }
        }
    }

    /// Drops the crossing under way, which changes nothing; steps of a count already crossed
    /// stay crossed. Returns whether one was under way.
    pub fn cancel_crossing(&mut self) -> bool {
        let Some(crossing) = self.crossing.under_way.take() else {
            return false;
        };
        self.crossing.freed.push(Box::new(crossing));
        true
    }

    /// What the crossings since the last call let go: the groups crossed, the text and display
    /// they replaced, and what a crossing cancelled or abandoned held. Dropping it may take
    /// milliseconds, so the host drops it off the UI thread (ADR 0119).
    pub fn take_crossing_freed(&mut self) -> Option<Box<dyn Send>> {
        if self.crossing.freed.is_empty() {
            return None;
        }
        Some(Box::new(std::mem::take(&mut self.crossing.freed)))
    }

    /// Crosses `count` steps at once, as [`Editor::undo`] and [`Editor::redo`] do: the slices
    /// back to back.
    pub(super) fn cross_at_once(&mut self, count: isize) -> Result<(), EditError> {
        self.begin_crossing(count)?;
        loop {
            if let Some(result) = self.go_on_crossing() {
                return result;
            }
        }
    }

    /// The next step of `id`'s history to cross `direction`, with `left` more after it.
    fn next_step(
        &self,
        id: BufferId,
        direction: Direction,
        left: usize,
    ) -> Result<Crossing, EditError> {
        let buffer = self.buffer(id).ok_or(EditError::NoSuchBuffer)?;
        let staged = buffer.stage_crossing(direction)?;
        // The selected window gets the cursors the step returns to, if it has them: only its
        // ring follows.
        let restores = buffer.crossing_selections(&staged).is_some();
        let selected = self.windows[self.selected].id();
        let windows = self
            .windows
            .iter()
            .filter(|window| window.buffer() == id)
            .map(|window| {
                let mut set = if restores && window.id() == selected {
                    SelectionSet::new(0).with_ring(*window.selections.mark_ring())
                } else {
                    window.selections.clone()
                };
                // A change hides every region of the buffer (ADR 0071).
                set.set_mark_active(MarkActive::Inactive);
                Followed {
                    id: window.id(),
                    before: window.selections.clone(),
                    set,
                    top: window.scroll().start,
                }
            })
            .collect();
        Ok(Crossing {
            buffer: id,
            window: self.windows[self.selected].id(),
            staged,
            windows,
            left,
            slices: 0,
            over: Duration::ZERO,
        })
    }

    /// What one slice may do, applying until `until`.
    fn crossing_budget(&self, until: Instant) -> Budget {
        #[cfg(test)]
        if let Some((work, piece)) = self.crossing.slices {
            return Budget::new(work, None, piece);
        }
        #[cfg(test)]
        let until = if self.crossing.no_time {
            Instant::now()
        } else {
            until
        };
        Budget::new(SLICE_WORK, Some(until), PIECE).with_sure(SURE_WORK)
    }

    /// Runs a slice of the step, and moves the windows' copies along. Returns whether the step
    /// has all applied.
    fn crossing_slice(
        &self,
        crossing: &mut Crossing,
        budget: &mut Budget,
    ) -> Result<bool, EditError> {
        let selected = &self.windows[self.selected];
        let showing = self
            .windows
            .iter()
            .filter(|window| window.buffer() == crossing.buffer);
        if selected.id() != crossing.window
            || selected.buffer() != crossing.buffer
            || !showing
                .map(|window| window.id())
                .eq(crossing.windows.iter().map(|followed| followed.id))
        {
            return Err(EditError::Abandoned);
        }
        let buffer = self.buffer(crossing.buffer).ok_or(EditError::Abandoned)?;
        let cursors: usize = crossing
            .windows
            .iter()
            .map(|followed| followed.set.len())
            .sum();
        budget.part = cursors * FOLLOW_WORK;
        let mut steps = Vec::new();
        let ended = buffer.apply_slice(&mut crossing.staged, budget, &mut steps)?;
        for followed in &mut crossing.windows {
            for step in &steps {
                followed.set.follow_edits(step, Bias::Left);
                followed.top = follow_offset(followed.top, step, Bias::Left);
            }
        }
        Ok(ended)
    }

    /// Commits a step that has all applied: the selected window gets the cursors of the side
    /// it returns to, and every other window showing the buffer keeps those that followed.
    /// What it lets go, or the step if it cannot commit, waits for the host to free.
    fn commit_step(&mut self, crossing: Crossing) -> Result<(), EditError> {
        let Crossing {
            buffer: id,
            staged,
            windows,
            ..
        } = crossing;
        let (moved, views) = match self.committed_windows(id, &staged, windows) {
            Ok(committed) => committed,
            Err(error) => {
                self.crossing.freed.push(Box::new(staged));
                return Err(error);
            }
        };

        // Nothing past this point can panic part way.
        let Some(buffer) = self.buffer_mut(id) else {
            self.crossing.freed.push(Box::new(staged));
            return Err(EditError::Abandoned);
        };
        match buffer.commit_crossing(staged) {
            Ok(freed) => self.crossing.freed.push(Box::new(freed)),
            Err(staged) => {
                self.crossing.freed.push(staged);
                return Err(EditError::Abandoned);
            }
        }
        for (index, set) in moved {
            self.windows[index].selections = set;
        }
        self.commit_views(id, views);
        // Crossing a recovery takes its verdict along.
        self.note_secret_shown();
        if let Some(scope) = &mut self.undo_scope {
            scope.before = (id, self.windows[self.selected].selections.clone());
            scope.crossed = true;
        }
        self.join_ready = false;
        Ok(())
    }

    /// Each window's cursors, by its index, and the views once `staged` is in, for
    /// [`Editor::commit_step`]. Fails if a window went or its cursors changed.
    fn committed_windows(
        &self,
        id: BufferId,
        staged: &StagedCrossing,
        windows: Vec<Followed>,
    ) -> Result<(Vec<(usize, SelectionSet)>, MovedViews), EditError> {
        let buffer = self.buffer(id).ok_or(EditError::Abandoned)?;
        let display = staged.display();
        let restored = buffer.crossing_selections(staged).cloned();
        let mut moved = Vec::with_capacity(windows.len());
        let mut tops = Vec::with_capacity(windows.len());
        for followed in windows {
            let index = self
                .windows
                .iter()
                .position(|window| window.id() == followed.id)
                .ok_or(EditError::Abandoned)?;
            if self.windows[index].selections != followed.before {
                return Err(EditError::Abandoned);
            }
            let mut set = match &restored {
                Some(restored) if index == self.selected => {
                    let mut set = restored.with_ring(*followed.set.mark_ring());
                    set.set_mark_active(MarkActive::Inactive);
                    set
                }
                _ => followed.set,
            };
            clip_selections(&mut set, &display, &[], true, Some(staged.changed()));
            moved.push((index, set));
            tops.push((followed.id, followed.top));
        }
        let views = self.moved_views_to(id, &display, staged.rewrap(), |window| {
            tops.iter()
                .find(|(other, _)| *other == window.id())
                .map_or(window.scroll().start, |(_, top)| *top)
        });
        Ok((moved, views))
    }

    /// The echo line while a crossing goes on, from its second slice: `Undoing… 42%`, with
    /// `(3 more)` while a count has more steps.
    pub(super) fn write_crossing_line(&self, out: &mut String) {
        let Some(crossing) = &self.crossing.under_way else {
            return;
        };
        if crossing.slices < 2 {
            return;
        }
        out.push_str(if crossing.redo() {
            "Redoing… "
        } else {
            "Undoing… "
        });
        // A share from 0 to 1, shown in whole percents.
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a share from 0 to 1 in whole percents"
        )]
        let percent = (crossing.done() * 100.0).floor() as u32;
        let _ = write!(out, "{percent}%");
        if crossing.left > 0 {
            let _ = write!(out, " ({} more)", crossing.left);
        }
    }

    /// Makes each slice's time up as it begins, so that it does only the work it is sure to.
    #[cfg(test)]
    pub(crate) fn set_crossing_no_time(&mut self) {
        self.crossing.no_time = true;
    }

    /// Makes each slice do `work`, as [`Budget`] counts it, with no time, and put back a
    /// splice's text `piece` bytes at a time, so that tests see the same slices every run.
    #[cfg(test)]
    pub(crate) fn set_crossing_slices(&mut self, work: usize, piece: usize) {
        self.crossing.slices = Some((work, piece));
    }
}

#[cfg(test)]
mod host_tests;
#[cfg(test)]
mod tests;
