//! What an action's key at a jump's labels does to the target of the label typed next, as
//! avy's `avy-dispatch-alist` (ADR 0116 step 6): `SPC` sets the region to it, `W`, `K`, `Y`
//! and `T` copy, kill, yank and move its whole line and stay, and `c` adds a cursor there.
//!
//! Must not depend on: `command` or `keymap`, or the host's clipboard, threads or I/O: a kill
//! goes through [`Editor::kill`]'s path, which leaves the clipboard to the host.
//!
//! - An action runs inside the command of the key that ended its label, so it is one undo
//!   step in each buffer it changes, as any command is.
//! - Every buffer it would change is asked first: a read-only one, or a yank past
//!   `kill::MAX_YANK`, changes nothing, the kill ring included.
//! - A kill from a buffer that may hold secrets is a secret entry, and a yank of it makes the
//!   buffer it goes into one too, as [`Editor::kill`] and [`Editor::yank`] have it (ADR 0074).
//! - A line's bounds come from the rope's line index, so an action costs O(log n) before the
//!   text a yank inserts, whatever the buffer's size.

use std::ops::Range;

use super::ActionError;
use crate::buffer::EditError;
use crate::editor::{CursorError, Editor, YankError};
use crate::jump::{Action, Candidate, Kind};
use crate::kill::{self, Direction, KillEntry};
use crate::rope::Rope;
use crate::selection::{MAX_CURSORS, MarkActive, Selection, SelectionSet};

impl Editor {
    /// Acts with `action` on `candidate`, which a jump of `kind` chose. `avy-isearch` ends its
    /// search first, as `RET` does, as avy ends it after the action.
    pub(super) fn jump_act(
        &mut self,
        kind: Kind,
        action: Action,
        candidate: &Candidate,
    ) -> Result<(), ActionError> {
        if kind == Kind::Isearch {
            self.isearch_exit();
        }
        match action {
            Action::Mark => {
                self.jump_mark(candidate);
                Ok(())
            }
            Action::Cursor => self.jump_add_cursor(candidate),
            Action::Copy | Action::Kill | Action::Yank | Action::Move => {
                self.jump_line(action, candidate)
            }
        }
    }

    /// Selects `candidate`'s window if it is another, and gives its index there and where its
    /// target is, on a display cluster's start.
    fn jump_target(&mut self, candidate: &Candidate) -> Option<(usize, usize)> {
        if candidate.window != self.tiled_id() {
            self.select_window(candidate.window);
        }
        let index = self.window_index(candidate.window)?;
        let (_, buffer) = self.window_mut(index);
        let offset = crate::motion::cluster_start(&buffer.display(), candidate.range.start);
        Some((index, offset))
    }

    /// `SPC`: `C-SPC` and then the jump. Every cursor's mark goes to it, the regions show, and
    /// the primary cursor goes to the target, so the region runs from where it was. In
    /// another window, that window's.
    fn jump_mark(&mut self, candidate: &Candidate) {
        let Some((index, offset)) = self.jump_target(candidate) else {
            return;
        };
        let selections = &mut self.window_mut(index).0.selections;
        selections.push_marks();
        selections.set_mark_active(MarkActive::Active);
        selections.move_primary(offset);
        self.follow_cursor();
    }

    /// `c`: adds a cursor at the target, with no mark, and the primary stays. In another
    /// window, to that window's cursors.
    fn jump_add_cursor(&mut self, candidate: &Candidate) -> Result<(), ActionError> {
        let full = |editor: &Self| {
            let index = editor.window_index(candidate.window);
            index.is_some_and(|index| editor.windows[index].selections.len() >= MAX_CURSORS)
        };
        if full(self) {
            return Err(ActionError::Cursor(CursorError::TooMany));
        }
        let Some((index, offset)) = self.jump_target(candidate) else {
            return Ok(());
        };
        // A run of the keys that add cursors starts again, as after `edit_lines`'s.
        self.drop_cursor_search();
        self.marking = None;
        let set = &self.windows[index].selections;
        let mut parts = set.as_slice().to_vec();
        parts.push(Selection::at(offset));
        let (primary, shown, ring) = (set.primary_index(), set.mark_active(), *set.mark_ring());
        let Some(mut added) = SelectionSet::from_parts(parts, primary) else {
            return Ok(());
        };
        added.set_mark_active(shown);
        self.windows[index].selections = added.with_ring(ring);
        Ok(())
    }

    /// `W`, `K`, `Y` and `T`: copies or kills the target's whole line, in its own window's
    /// buffer, and for `Y` and `T` yanks it at every cursor of the selected window as
    /// `C-u C-y` does. The selected window and its cursors stay, but for what the edits move.
    fn jump_line(&mut self, action: Action, candidate: &Candidate) -> Result<(), ActionError> {
        let Some(source) = self.window_by_id(candidate.window).map(|w| w.buffer()) else {
            return Ok(());
        };
        let Some(buffer) = self.buffer(source) else {
            return Ok(());
        };
        let read_only = |reason| ActionError::Edit(EditError::ReadOnly(reason));
        if action.kills()
            && let Some(reason) = buffer.read_only()
        {
            return Err(read_only(reason));
        }
        let range = whole_line(buffer.text(), candidate.range.start, action.kills());
        if action.yanks() {
            let (window, here) = self.current();
            if let Some(reason) = here.read_only() {
                return Err(read_only(reason));
            }
            let total = range.len().checked_mul(window.selections.len());
            if total.is_none_or(|total| total > kill::MAX_YANK) {
                return Err(ActionError::Yank(YankError::TooMuch));
            }
        }
        let newest = self.kill_ring().get(0).map(KillEntry::id);
        // Its own entry, as avy's action follows no kill.
        self.forget_kills();
        self.kill_in(
            source,
            std::iter::once(range),
            Direction::Forward,
            !action.kills(),
        )
        .map_err(ActionError::Edit)?;
        // An empty line's end gives nothing to yank, and an older entry must not go in.
        if action.yanks() && self.kill_ring().get(0).map(KillEntry::id) != newest {
            self.yank(0, true).map_err(ActionError::Yank)?;
        }
        Ok(())
    }
}

/// The whole line of `text` at `offset`, from its start through its line break, as the
/// author's avy actions take it. A last line with no break has only its text, unless `kills`,
/// when it takes the break before it too, as Emacs's `kill-whole-line` does, so that no empty
/// line is left.
fn whole_line(text: &Rope, offset: usize, kills: bool) -> Range<usize> {
    let row = text.offset_to_point(offset).row;
    let start = text.line_start(row);
    let next = text.line_start(row + 1);
    let has_break = next > text.line_end(row);
    match row.checked_sub(1) {
        Some(above) if kills && !has_break => text.line_end(above)..next,
        _ => start..next,
    }
}

#[cfg(test)]
mod tests {
    use super::whole_line;
    use crate::rope::Rope;

    #[test]
    fn a_whole_line_runs_through_its_break_and_a_last_kill_takes_the_one_before() {
        let text = Rope::from("one\r\ntwo\nlast");
        assert_eq!(whole_line(&text, 1, false), 0..5, "with its CRLF");
        assert_eq!(whole_line(&text, 5, true), 5..9);
        assert_eq!(whole_line(&text, 11, false), 9..13, "its text alone");
        assert_eq!(whole_line(&text, 11, true), 8..13, "and the break before");
        let one = Rope::from("only");
        assert_eq!(whole_line(&one, 2, true), 0..4);
        let ends = Rope::from("a\n");
        assert_eq!(whole_line(&ends, 2, false), 2..2, "an empty last line");
        assert_eq!(whole_line(&ends, 2, true), 1..2);
    }
}
