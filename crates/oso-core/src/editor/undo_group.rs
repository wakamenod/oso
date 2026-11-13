//! The undo group of a command (ADR 0053): opening it before the command runs, joining or
//! keeping apart what follows, closing it after, also after a panic, and undoing or redoing
//! a step with the selected window's cursors as they were.
//!
//! Must not depend on: `command`, `keymap`, or the host's window, threads or I/O.

use super::{Editor, UndoScope};
use crate::buffer::{Buffer, BufferId, EditError};
use crate::selection::SelectionSet;
use crate::undo::Opening;

impl Editor {
    /// Opens the undo group of a command. The dispatcher calls it before it
    /// runs a command. Edits until [`Editor::end_undo_group`] undo as one step,
    /// which returns to the selected window's cursors as they are now. A group
    /// left open closes here first, so a missed end costs one step, not all
    /// later ones in a single group.
    ///
    /// Each window's wrap gets its budget for the command here too (ADR 0102).
    pub fn begin_undo_group(&mut self) {
        self.refill_wraps();
        self.close_undo_scope(false);
        let window = &self.windows[self.selected];
        self.undo_scope = Some(UndoScope {
            before: (window.buffer(), window.selections.clone()),
            touched: Vec::new(),
            join: false,
            apart: false,
            crossed: false,
        });
    }

    /// Lets the edits of the running command join the previous command's undo
    /// group, if that command called this too, edited something and undid
    /// nothing, and the cursors are where it left them. For self-inserted characters, up to a limit (ADR 0030). Does
    /// nothing with no group open.
    pub fn allow_undo_join(&mut self) {
        if let Some(scope) = &mut self.undo_scope {
            scope.join = !scope.apart;
        }
    }

    /// Keeps the running command's edits a step of their own, joining neither the
    /// step before nor the one after, whatever the command asks. An input method's
    /// commit is one, even of a single character (ADR 0069). Does nothing with no
    /// group open.
    pub fn keep_undo_step_apart(&mut self) {
        if let Some(scope) = &mut self.undo_scope {
            scope.apart = true;
            scope.join = false;
        }
    }

    /// Closes the undo group [`Editor::begin_undo_group`] opened. The group
    /// ends with the selected window's cursors as they are now, so a command may
    /// move them after its edits. Does nothing with no group open.
    pub fn end_undo_group(&mut self) {
        self.close_undo_scope(true);
    }

    /// Closes the open undo group when its command panicked. What the command
    /// already applied stays in the buffer as one undo step, and nothing joins
    /// it. It never panics, and does nothing with no group open.
    pub fn end_undo_group_after_panic(&mut self) {
        self.close_undo_scope(false);
    }

    fn close_undo_scope(&mut self, may_join: bool) {
        let Some(scope) = self.undo_scope.take() else {
            return;
        };
        self.join_ready = may_join && scope.join && !scope.crossed && !scope.touched.is_empty();
        for id in scope.touched {
            self.seal_undo_group(id);
        }
    }

    /// Records the selected window's cursors as the end of the newest undo
    /// group of `id`, if it shows that buffer.
    pub(super) fn seal_undo_group(&mut self, id: BufferId) {
        if let Some(selections) = self.selected_selections(id).cloned()
            && let Some(buffer) = self.buffer_mut(id)
        {
            buffer.set_undo_after(selections);
        }
    }

    /// Undoes the newest undo group of the selected window's buffer. The window
    /// gets the cursors from before the group, and other windows' cursors follow
    /// the text. The buffer's content id returns to the one before the group.
    /// It crosses the whole step at once, dropping a crossing under way, and what it lets go
    /// waits for [`Editor::take_crossing_freed`] (ADR 0119).
    ///
    /// # Errors
    ///
    /// [`EditError::NoUndo`] and [`EditError::ReadOnly`]. Nothing changes.
    pub fn undo(&mut self) -> Result<(), EditError> {
        self.cross_at_once(1)
    }

    /// Redoes the group the last undo crossed, if no edit came since (ADR
    /// 0030). The selected window gets the cursors from after the group. As
    /// [`Editor::undo`], it crosses at once.
    ///
    /// # Errors
    ///
    /// [`EditError::NoRedo`] and [`EditError::ReadOnly`]. Nothing changes.
    pub fn redo(&mut self) -> Result<(), EditError> {
        self.cross_at_once(-1)
    }
}

impl Editor {
    /// `id` has been edited: the open undo group touches it, or with none, nothing joins.
    pub(super) fn note_edited(&mut self, id: BufferId) {
        match &mut self.undo_scope {
            Some(scope) if !scope.touched.contains(&id) => scope.touched.push(id),
            Some(_) => {}
            None => self.join_ready = false,
        }
    }

    /// How an edit of `id` relates to its newest undo group.
    pub(super) fn opening(&self, id: BufferId, buffer: &Buffer) -> Opening {
        let Some(scope) = &self.undo_scope else {
            return Opening::New(self.selected_selections(id).cloned());
        };
        if scope.touched.contains(&id) {
            return Opening::Continue;
        }
        let before = if scope.before.0 == id {
            Some(&scope.before.1)
        } else {
            self.selected_selections(id)
        };
        if scope.join && self.join_ready && buffer.can_join_undo(before) {
            Opening::Join
        } else {
            Opening::New(before.cloned())
        }
    }

    /// The selected window's cursors, if it shows `id`.
    pub(super) fn selected_selections(&self, id: BufferId) -> Option<&SelectionSet> {
        let window = &self.windows[self.selected];
        (window.buffer() == id).then_some(&window.selections)
    }
}
