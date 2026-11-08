//! Files changed on disk (ADR 0029, ADR 0083): a buffer with no unsaved edits takes in its
//! file read again, as one undo step, with every window's cursors kept where they were; one
//! with edits keeps them and warns.
//!
//! Must not depend on: `command` or `keymap`, or the host's threads or files; the host
//! notices the change, reads the file and hands over what it found.

use std::path::PathBuf;

use super::Editor;
use super::follow::clip_selections;
use crate::buffer::{BufferId, ContentId, DiskState, Noticed};
use crate::display_map::DisplaySnapshot;
use crate::load::{Change, LoadSnapshot, Reloaded};
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
    /// The file is now binary or not valid UTF-8, whose text cannot be edited or undone, and
    /// the buffer has unsaved edits: nothing went in.
    NotEditable,
    /// No buffer has the id.
    NoSuchBuffer,
}

/// Text a reload left: the one before it, or the one read and not put in. Dropping it can
/// take long for a large file, so the host drops it on another thread.
#[expect(dead_code, reason = "held only to be dropped where the host says")]
pub struct Leftover {
    text: Option<Rope>,
    display: Option<DisplaySnapshot>,
    snapshot: Option<LoadSnapshot>,
}

impl Leftover {
    fn of_snapshot(snapshot: LoadSnapshot) -> Self {
        Self {
            text: None,
            display: None,
            snapshot: Some(snapshot),
        }
    }
}

/// Where an offset of the old text goes in the new one. Before the change and after it, it
/// keeps its place, and inside it, its line and column, as far as the new line reaches and no
/// further than the change.
fn through_reload(offset: usize, old: &Rope, new: &Rope, change: Change) -> usize {
    if offset <= change.start {
        offset
    } else if offset >= change.old_end {
        offset - change.old_end + change.new_end
    } else {
        // A line gone past the change would put it in the text after.
        new.point_to_offset(old.offset_to_point(offset))
            .clamp(change.start, change.new_end)
    }
}

impl Editor {
    /// The file of `id` could not be read again: the modeline says it changed on disk, edits or
    /// not, and the next look tries again.
    pub fn reload_failed(&mut self, id: BufferId) {
        if let Some(buffer) = self.buffer_mut(id) {
            buffer.set_reload_failed();
        }
    }

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
    /// [`ReloadOutcome::Reloaded`] and [`ReloadOutcome::Same`]. The [`Leftover`], the text
    /// before or the one refused, is the host's to free off the UI thread, as a large file's
    /// would take long to.
    pub fn reload(
        &mut self,
        id: BufferId,
        reloaded: Reloaded,
        read_from: ContentId,
    ) -> (ReloadOutcome, Leftover) {
        self.put_reload_in(id, reloaded, read_from, false)
    }

    /// Puts in the text of `id`'s file read again as [`Editor::reload`] does, over unsaved edits
    /// too, as a person asked with [`Editor::request_revert`]. The edits are one undo away.
    pub fn revert(
        &mut self,
        id: BufferId,
        reloaded: Reloaded,
        read_from: ContentId,
    ) -> (ReloadOutcome, Leftover) {
        self.put_reload_in(id, reloaded, read_from, true)
    }

    /// Asks the host to read the selected window's buffer again from its file, discarding its
    /// unsaved edits as they are now, for `revert-buffer`.
    pub fn request_revert(&mut self) {
        let id = self.window().buffer();
        let content = self.current().1.content_id();
        self.queue_revert(id, content);
    }

    fn queue_revert(&mut self, id: BufferId, content: ContentId) {
        self.reverts.retain(|(other, _)| *other != id);
        self.reverts.push((id, content));
    }

    /// The buffers asked to be read again that may be now, each with the state its read is to
    /// replace. One waits while [`Editor::reload_waits`], or while its buffer loads or saves.
    /// One whose buffer changed since it was asked for is dropped and said so, as the edits
    /// made since were not what the person agreed to lose.
    pub fn take_revert_requests(&mut self) -> Vec<(BufferId, ContentId)> {
        let mut ready = Vec::new();
        if self.reverts.is_empty() || self.reload_waits() {
            return ready;
        }
        let mut said = Vec::new();
        let requests = std::mem::take(&mut self.reverts);
        for (id, content) in requests {
            let Some(buffer) = self.buffer(id) else {
                continue;
            };
            if buffer.loading().is_some() || buffer.is_saving() {
                self.reverts.push((id, content));
            } else if buffer.content_id() == content {
                ready.push((id, content));
            } else {
                said.push(format!(
                    "Not reloaded, as {} changed meanwhile",
                    buffer.name()
                ));
            }
        }
        for message in said {
            self.message(message);
        }
        ready
    }

    fn put_reload_in(
        &mut self,
        id: BufferId,
        reloaded: Reloaded,
        read_from: ContentId,
        discard_edits: bool,
    ) -> (ReloadOutcome, Leftover) {
        let waits = self.reload_waits();
        let Reloaded { snapshot, change } = reloaded;
        let Some(buffer) = self.buffer(id) else {
            return (ReloadOutcome::NoSuchBuffer, Leftover::of_snapshot(snapshot));
        };
        let later = waits || buffer.loading().is_some() || buffer.is_saving();
        let edited = buffer.content_id() != read_from || (buffer.is_modified() && !discard_edits);
        if later || edited {
            if let Some(buffer) = self.buffer_mut(id) {
                buffer.forget_disk_action();
            }
            if later {
                // A person asked for it, so it is asked for again, for the same state.
                if discard_edits {
                    self.queue_revert(id, read_from);
                }
                return (ReloadOutcome::Later, Leftover::of_snapshot(snapshot));
            }
            self.file_changed_on_disk(id);
            return (ReloadOutcome::Edited, Leftover::of_snapshot(snapshot));
        }
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
                return (ReloadOutcome::Same, Leftover::of_snapshot(snapshot));
            }
            None => Change {
                start: 0,
                old_end: old.len(),
                new_end: snapshot.text.len(),
            },
            Some(change) => change,
        };
        // The text before, kept until the host frees it off the UI thread.
        let leftover = Leftover {
            text: Some(old.clone()),
            display: Some(buffer.display()),
            snapshot: None,
        };
        let display = snapshot.display.snapshot();
        let moved = self.reloaded_selections(id, &old, &snapshot.text, &display, change);
        let staged = buffer.stage_reload(&snapshot, change);
        if staged.is_none() && buffer.is_modified() {
            // The edits would go with no undo to bring them back.
            return (ReloadOutcome::NotEditable, Leftover::of_snapshot(snapshot));
        }
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
        (ReloadOutcome::Reloaded, leftover)
    }

    /// Takes in what the host's looks at files found, as `(buffer, state)` sorted by buffer,
    /// and returns the buffers whose files are to be read again: those whose file changed under
    /// no unsaved edits. A file that changed under edits, or went away, warns on the modeline and
    /// says so once in the echo area. Each thing seen is acted on once, but for a read that
    /// failed, which is tried again when `seen` is `fresh`, a new look. `busy` says which buffers
    /// a read already runs for. Nothing is done while [`Editor::reload_waits`]. O(buffers log
    /// seen), and allocates only to act.
    pub fn files_seen(
        &mut self,
        seen: &[(BufferId, DiskState)],
        fresh: bool,
        busy: impl Fn(BufferId) -> bool,
    ) -> Vec<BufferId> {
        let mut reads = Vec::new();
        if self.reload_waits() {
            return reads;
        }
        let mut said = Vec::new();
        for (id, buffer) in &mut self.buffers {
            if buffer.is_internal() || busy(*id) {
                continue;
            }
            let Ok(index) = seen.binary_search_by_key(id, |(seen, _)| *seen) else {
                continue;
            };
            if fresh && buffer.reload_failed() {
                buffer.forget_disk_action();
            }
            match buffer.file_seen(seen[index].1) {
                Some(Noticed::Reload) => reads.push(*id),
                Some(Noticed::Changed) => {
                    said.push(format!(
                        "{} changed on disk; your edits are kept",
                        buffer.name()
                    ));
                }
                Some(Noticed::Gone) => {
                    said.push(format!(
                        "{} is gone from disk; its text is kept",
                        buffer.name()
                    ));
                }
                None => {}
            }
        }
        for message in said {
            self.message(message);
        }
        reads
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
