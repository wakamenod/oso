//! Pairs of windows that scroll together (ADR 0122, step 10): two tiled windows, side by side or
//! stacked, whose lines line up by a row map their owner gives, closed and resized together.
//!
//! Must not depend on: `command`, `keymap` or the host's window. A pair is generic, with nothing
//! of what its owner shows: the owner builds both buffers and the map, and this keeps the windows
//! in step.
//!
//! The tree holds a pair as one run of two windows, which it splits, resizes and deletes as one
//! (`frame::Splits`). This keeps the map, and after every command and every turn of the wheel
//! moves the other window to where the one that moved is: its top to the start of the line
//! aligned with the leader's top, its left column with the leader's, and after a command its
//! primary cursor to the line aligned with the leader's, alone, at the same display column
//! clamped to that line, its goal kept. The wheel moves the other's top only, and its cursors
//! into view as the wheel moves any window's. A pair holds while each window shows the buffer
//! it was paired with: showing another breaks it, and both windows stay. The restart handoff
//! carries a pair as two windows of their own, which the owner pairs again (ADR 0122, Q7).

use std::fmt;

use super::Editor;
use crate::buffer::BufferId;
use crate::display_map::DisplayPoint;
use crate::frame::{TooSmall, Way};
use crate::motion;
use crate::selection::Goal;
use crate::sum_tree::Bias;
use crate::window::{RowMap, Scroll, ViewTop, WindowId};

/// [`Editor::pair_window`] refused. Nothing changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoPair {
    /// Either window would be smaller than a split leaves.
    TooSmall,
    /// The selected window is in a pair already.
    Paired,
    /// There is no such buffer.
    NoBuffer,
}

impl fmt::Display for NoPair {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooSmall => fmt::Display::fmt(&TooSmall, f),
            Self::Paired => f.write_str("The window is in a pair already"),
            Self::NoBuffer => f.write_str("No such buffer"),
        }
    }
}

impl std::error::Error for NoPair {}

/// A pair's windows, what each was paired to show, and how their lines line up.
pub(super) struct Pair {
    /// The first, above or at the left, then the second.
    windows: [WindowId; 2],
    buffers: [BufferId; 2],
    map: RowMap,
    /// Each window's view and primary cursor as the last move in step left them, so that a
    /// window that did not move since leads nothing.
    seen: [Option<(Scroll, usize)>; 2],
}

/// What the leader of a pair moves in the other window.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Lead {
    /// After a command: the view and the primary cursor.
    Cursor,
    /// After the wheel: the view, the cursors following into it unless they `stay`.
    View { stay: bool },
}

impl Editor {
    /// Splits the selected tiled window into a pair, the new window at the right with
    /// [`Way::Across`] or below with [`Way::Down`], showing `buffer`, its lines lining up with
    /// the selected window's by `map`. The selected window stays selected and leads at once.
    /// Returns the new window's name. Ends isearch.
    ///
    /// # Errors
    ///
    /// [`NoPair`] for a buffer that does not exist, a window in a pair already, or halves
    /// smaller than a split leaves. Nothing changes.
    pub fn pair_window(
        &mut self,
        way: Way,
        buffer: BufferId,
        map: RowMap,
    ) -> Result<WindowId, NoPair> {
        if self.buffer(buffer).is_none() {
            return Err(NoPair::NoBuffer);
        }
        let first = self.tiled_id();
        if self.pair_at(first).is_some() {
            return Err(NoPair::Paired);
        }
        let id = WindowId::new(self.next_window);
        self.set_split_gutter();
        let mut splits = self.splits.clone();
        splits
            .pair(first, way, id, &self.frame)
            .map_err(|TooSmall| NoPair::TooSmall)?;
        self.isearch_exit();
        self.splits = splits;
        self.next_window += 1;
        self.add_split_off(self.tiled_index(), id);
        self.lay_out(self.frame.size());
        if let Some(index) = self.window_index(id) {
            self.window_shows(index, buffer);
        }
        let first_buffer = self.windows[self.tiled_index()].buffer();
        self.pairs.push(Pair {
            windows: [first, id],
            buffers: [first_buffer, buffer],
            map,
            seen: [None; 2],
        });
        self.lead_pair(first, Lead::Cursor);
        Ok(id)
    }

    /// The other window of the pair `id` is in, if it is in one.
    pub fn pair_of(&self, id: WindowId) -> Option<WindowId> {
        let pair = &self.pairs[self.pair_at(id)?];
        pair.windows.into_iter().find(|other| *other != id)
    }

    /// Gives the pair `id` is in a new map, as its owner does when the lines change, and moves
    /// its windows in step by it at once, from the selected one, or from the first while
    /// neither is. Returns whether `id` is in a pair.
    pub fn set_row_map(&mut self, id: WindowId, map: RowMap) -> bool {
        let Some(at) = self.pair_at(id) else {
            return false;
        };
        self.pairs[at].map = map;
        self.pairs[at].seen = [None; 2];
        self.lead_from_either(at);
        true
    }

    /// Lays the pair `id` is in out side by side with [`Way::Across`] or stacked with
    /// [`Way::Down`], its first window still at the left or above, and moves its windows in step
    /// again. Returns whether it changed: not for a window in no pair, nor a pair laid out so.
    ///
    /// # Errors
    ///
    /// [`TooSmall`] when either window would be smaller than a split leaves. Nothing changes.
    pub fn turn_pair(&mut self, id: WindowId, way: Way) -> Result<bool, TooSmall> {
        self.set_split_gutter();
        if !self.splits.turn(id, way, &self.frame)? {
            return Ok(false);
        }
        self.lay_out(self.frame.size());
        if let Some(at) = self.pair_at(id) {
            self.pairs[at].seen = [None; 2];
            self.lead_from_either(at);
        }
        Ok(true)
    }

    /// Closes the window paired with `keep`, which takes the pair's cells as a window of its
    /// own, selected if the other was. Returns whether `keep` was in a pair.
    pub fn unpair(&mut self, keep: WindowId) -> bool {
        let Some(other) = self.splits.unpair(keep) else {
            return false;
        };
        if self.tiled_id() == other {
            self.isearch_exit();
        }
        self.drop_window(other);
        self.lay_out(self.frame.size());
        true
    }

    /// The index of the pair `id` is in.
    fn pair_at(&self, id: WindowId) -> Option<usize> {
        self.pairs
            .iter()
            .position(|pair| pair.windows.contains(&id))
    }

    /// Breaks the pair of the window at `index` if it is about to show `buffer`, which it was
    /// not paired with: both windows stay, as windows of their own.
    pub(super) fn leave_pair(&mut self, index: usize, buffer: BufferId) {
        let id = self.windows[index].id();
        let Some(at) = self.pair_at(id) else {
            return;
        };
        let side = usize::from(self.pairs[at].windows[1] == id);
        if self.pairs[at].buffers[side] != buffer {
            self.splits.break_pair(id, &self.frame);
            self.pairs.remove(at);
        }
    }

    /// Forgets the pair of `id`, once the window is gone.
    pub(super) fn forget_pair(&mut self, id: WindowId) {
        self.pairs.retain(|pair| !pair.windows.contains(&id));
    }

    /// After a command: the selected tiled window, if it is in a pair and moved since its
    /// windows were last in step, moves the other. Returns whether the other's view moved.
    pub(super) fn follow_pair(&mut self) -> bool {
        let id = self.tiled_id();
        self.lead_pair(id, Lead::Cursor)
    }

    /// After the wheel scrolled `id`: the other window of its pair scrolls with it, its cursors
    /// into its view unless they stay, as the wheel leaves those of the window beneath a prompt
    /// and of one with a preedit (ADR 0072, ADR 0069). Returns whether its view moved.
    pub(super) fn wheel_pair(&mut self, id: WindowId) -> bool {
        let Some(other) = self.pair_of(id) else {
            return false;
        };
        let stay = self.tiled_id() == other && (self.preedit.is_some() || !self.prompts.is_empty());
        self.lead_pair(id, Lead::View { stay })
    }

    /// Moves the pair at `at` in step from its selected window, or its first.
    fn lead_from_either(&mut self, at: usize) {
        let selected = self.tiled_id();
        let windows = self.pairs[at].windows;
        let leader = if windows.contains(&selected) {
            selected
        } else {
            windows[0]
        };
        self.lead_pair(leader, Lead::Cursor);
    }

    /// Moves the other window of the pair `leader` is in to where `leader` is, if `leader`
    /// moved since they were last in step. Returns whether the other's view moved.
    fn lead_pair(&mut self, leader: WindowId, lead: Lead) -> bool {
        let Some(at) = self.pair_at(leader) else {
            return false;
        };
        let side = usize::from(self.pairs[at].windows[1] == leader);
        let other = self.pairs[at].windows[1 - side];
        let (Some(from), Some(to)) = (self.window_index(leader), self.window_index(other)) else {
            return false;
        };
        let now = self.pair_seen(from);
        if self.pairs[at].seen[side] == Some(now) && lead == Lead::Cursor {
            return false;
        }
        // Where the leader is, in its lines.
        let (window, buffer) = (&self.windows[from], self.shown(&self.windows[from]));
        let display = buffer.display();
        let top = display.offset_to_display_point(window.scroll().start).row;
        let cursor = window.cursor_point(&display);
        let goal = window.selections.primary().goal;
        let left = window.scroll().left;
        let map = &self.pairs[at].map;
        let across = |line: usize| {
            if side == 0 {
                map.second_of(line)
            } else {
                map.first_of(line)
            }
        };
        let (top, line) = (across(top), across(cursor.row));
        let before = self.windows[to].scroll();
        match lead {
            Lead::Cursor => self.follow_lead(
                to,
                (top, left),
                line,
                goal.or(Some(Goal::Column(cursor.column))),
            ),
            Lead::View { stay } => self.view_lead(to, (top, left), stay),
        }
        let moved = self.windows[to].scroll() != before;
        let (leader_seen, other_seen) = (self.pair_seen(from), self.pair_seen(to));
        let seen = &mut self.pairs[at].seen;
        seen[side] = Some(leader_seen);
        seen[1 - side] = Some(other_seen);
        moved
    }

    /// The window at `index`'s view and primary cursor, as a pair's last step keeps them.
    fn pair_seen(&self, index: usize) -> (Scroll, usize) {
        let window = &self.windows[index];
        (window.scroll(), window.selections.primary().cursor)
    }

    /// Shows the window at `index` from the start of line `top`, at column `left` while it
    /// does not wrap, with its primary cursor alone on line `line` where `goal` aims, its mark
    /// kept; then scrolls it as little as shows the cursor, which a narrower wrap may need.
    fn follow_lead(
        &mut self,
        index: usize,
        (top, left): (usize, usize),
        line: usize,
        goal: Option<Goal>,
    ) {
        let (window, buffer) = self.window_mut(index);
        let display = buffer.display();
        let last = display.row_count().saturating_sub(1);
        let line = line.min(last);
        let offset = match goal {
            Some(Goal::LineEnd) => motion::line_end(&display, line),
            Some(Goal::Column(column)) => {
                display.display_point_to_offset(DisplayPoint::new(line, column), Bias::Left)
            }
            None => motion::line_start(&display, line),
        };
        window.selections.keep_primary();
        window.selections.update(|selection| {
            selection.cursor = offset;
            selection.goal = goal;
            Ok::<(), ()>(())
        });
        let start = motion::line_start(&display, top.min(last));
        let left = if window.rows(&display).wrapped() {
            0
        } else {
            left
        };
        window.set_scroll(Scroll { start, left });
        self.follow_window(index);
    }

    /// Shows the window at `index` from the start of line `top`, at column `left` while it
    /// does not wrap, its cursors following into the view as the wheel's do unless they `stay`.
    fn view_lead(&mut self, index: usize, (top, left): (usize, usize), stay: bool) {
        self.pass_wrap_budget(index);
        let (window, buffer) = self.window_mut(index);
        let display = buffer.display();
        let start = motion::line_start(&display, top.min(display.row_count().saturating_sub(1)));
        window.realize(&display, start, window.size().rows);
        let row = window.rows(&display).row_of(start);
        if stay {
            window.set_view_top(ViewTop { row, left }, &display);
        } else {
            window.set_top(row, &display);
            if !window.rows(&display).wrapped() {
                let scroll = window.scroll();
                window.set_scroll(Scroll { left, ..scroll });
            }
        }
        window.realize_view(&display);
    }
}

#[cfg(test)]
mod tests;
