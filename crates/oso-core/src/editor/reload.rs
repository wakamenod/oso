//! Files changed on disk (ADR 0029, ADR 0083): a buffer with no unsaved edits takes in its
//! file read again, as one undo step, with every window's cursors kept where they were; one
//! with edits keeps them and warns.
//!
//! Must not depend on: `command` or `keymap`, or the host's threads or files; the host
//! notices the change, reads the file and hands over what it found.

use std::path::PathBuf;

use super::Editor;
use super::follow::clip_selections;
use crate::buffer::{BufferId, ContentId};
use crate::display_map::DisplaySnapshot;
use crate::load::{Change, Reloaded};
use crate::rope::Rope;
use crate::selection::{MarkActive, SelectionSet};

/// What became of a file read again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReloadOutcome {
    /// The new text is in, as one undo step, and the buffer is unmodified.
    Reloaded,
    /// The text is what the buffer holds. Only what the host records of the file changes.
    Same,
    /// The buffer changed since the read began, or has unsaved edits: nothing went in, and
    /// its file is now known to have changed.
    Edited,
    /// Not now, as [`Editor::reload_waits`] says, or the buffer loads or saves. Nothing went
    /// in, and a read later may.
    Later,
    /// No buffer has the id.
    NoSuchBuffer,
}

/// Where an offset of the old text goes in the new one. Before the change and after it, it
/// keeps its place, and inside it, its line and column, as far as the new line reaches.
fn through_reload(offset: usize, old: &Rope, new: &Rope, change: Change) -> usize {
    if offset <= change.start {
        offset
    } else if offset >= change.old_end {
        offset - change.old_end + change.new_end
    } else {
        new.point_to_offset(old.offset_to_point(offset))
    }
}

impl Editor {
    /// Whether a file read again must wait: while a prompt is open, as what it acts on must
    /// not change under it (ADR 0072).
    pub fn reload_waits(&self) -> bool {
        !self.prompts.is_empty()
    }

    /// Judges the file of `id` again, as it is being read again: from where its path leads now
    /// (`Ok`), or from what the walk passed when nobody could tell (`Err`). It never withholds
    /// less than before. The host calls this before it hands over the text.
    pub fn judge_reload(&mut self, id: BufferId, resolved: Result<&[PathBuf], &[PathBuf]>) {
        if let Some(buffer) = self.buffer_mut(id) {
            buffer.judge_again(resolved);
        }
        self.note_secret_shown();
    }

    /// Puts in the text of `id`'s file read again, if the buffer still holds the state
    /// `read_from` it held when the read began and has no unsaved edits. The change is one undo
    /// step, after which the buffer is unmodified, and undo brings back the text before it. A
    /// text that cannot be edited, as a binary file's, replaces the old one with no undo. Every
    /// window's cursors and marks keep their place outside the change and their line and
    /// column inside it, and each window keeps its top row.
    ///
    /// What the host records of the file, the baseline, is the host's to set on
    /// [`ReloadOutcome::Reloaded`] and [`ReloadOutcome::Same`].
    pub fn reload(
        &mut self,
        id: BufferId,
        reloaded: Reloaded,
        read_from: ContentId,
    ) -> ReloadOutcome {
        self.put_reload_in(id, reloaded, read_from, false)
    }

    /// Puts in the text of `id`'s file read again as [`Editor::reload`] does, over unsaved edits
    /// too, as a person asked with [`Editor::request_revert`]. The edits are one undo away.
    pub fn revert(
        &mut self,
        id: BufferId,
        reloaded: Reloaded,
        read_from: ContentId,
    ) -> ReloadOutcome {
        self.put_reload_in(id, reloaded, read_from, true)
    }

    /// Asks the host to read the selected window's buffer again from its file, discarding its
    /// unsaved edits, for `revert-buffer`.
    pub fn request_revert(&mut self) {
        let id = self.window().buffer();
        if !self.reverts.contains(&id) {
            self.reverts.push(id);
        }
    }

    /// The buffers asked to be read again since the last call, for the host.
    pub fn take_revert_requests(&mut self) -> Vec<BufferId> {
        std::mem::take(&mut self.reverts)
    }

    fn put_reload_in(
        &mut self,
        id: BufferId,
        reloaded: Reloaded,
        read_from: ContentId,
        discard_edits: bool,
    ) -> ReloadOutcome {
        let waits = self.reload_waits();
        let Some(buffer) = self.buffer(id) else {
            return ReloadOutcome::NoSuchBuffer;
        };
        if waits || buffer.loading().is_some() || buffer.is_saving() {
            return ReloadOutcome::Later;
        }
        if buffer.content_id() != read_from || (buffer.is_modified() && !discard_edits) {
            self.file_changed_on_disk(id);
            return ReloadOutcome::Edited;
        }
        let Reloaded { snapshot, change } = reloaded;
        let old = buffer.text().clone();
        let change = match change {
            None if snapshot.display.invalid_bytes().is_empty()
                && buffer.invalid_bytes().is_empty() =>
            {
                if let Some(buffer) = self.buffer_mut(id) {
                    buffer.set_file_info(snapshot.info);
                    // Edits that came back to the same text are no longer edits.
                    buffer.mark_saved(buffer.content_id());
                    buffer.reloaded();
                }
                return ReloadOutcome::Same;
            }
            None => Change {
                start: 0,
                old_end: old.len(),
                new_end: snapshot.text.len(),
            },
            Some(change) => change,
        };
        let display = snapshot.display.snapshot();
        let moved = self.reloaded_selections(id, &old, &snapshot.text, &display, change);
        let staged = buffer.stage_reload(&snapshot, change);
        let opening = self.opening(id, buffer);
        let after = moved
            .iter()
            .find_map(|(index, set)| (*index == self.selected).then(|| set.clone()));

        // Nothing past this point can panic part way.
        if let Some(buffer) = self.buffer_mut(id) {
            match staged {
                Some(staged) => buffer.commit_reload(staged, &snapshot, opening, after),
                None => buffer.replace_with_reload(snapshot),
            }
        }
        for (index, set) in moved {
            let scroll = self.windows[index].scroll();
            self.windows[index].restore_positions(set, scroll, &display);
        }
        let shown: Vec<usize> = (0..self.windows.len())
            .filter(|index| self.windows[*index].buffer() == id)
            .collect();
        for index in shown {
            self.follow_window(index);
        }
        self.note_edited(id);
        self.note_secret_shown();
        ReloadOutcome::Reloaded
    }

    /// The file of `id` was found gone, or no longer a file: its text stays, and the modeline
    /// says so until the file is back or a save writes it.
    pub fn file_gone_from_disk(&mut self, id: BufferId) {
        if let Some(buffer) = self.buffer_mut(id) {
            buffer.set_gone_from_disk(true);
        }
    }

    /// The cursors of every window showing `id` once the text read again is in, by window
    /// position. A change hides every region of the buffer (ADR 0071).
    fn reloaded_selections(
        &self,
        id: BufferId,
        old: &Rope,
        new: &Rope,
        display: &DisplaySnapshot,
        change: Change,
    ) -> Vec<(usize, SelectionSet)> {
        let map = |offset| through_reload(offset, old, new, change);
        let mut moved = Vec::new();
        for (index, window) in self.windows.iter().enumerate() {
            if window.buffer() != id {
                continue;
            }
            let mut set = window.selections.clone();
            set.set_mark_active(MarkActive::Inactive);
            set.update(|selection| {
                selection.cursor = map(selection.cursor);
                selection.mark = selection.mark.map(map);
                Ok::<(), ()>(())
            });
            set.update_ring(map);
            clip_selections(&mut set, display, &[], true);
            moved.push((index, set));
        }
        moved
    }
}
