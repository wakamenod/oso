//! A buffer's file read again after it changed on disk (ADR 0029, ADR 0083): what is known of
//! the file's state, as a look or a save found it, the verdict judged again, and the new text
//! put in as one undo step.
//!
//! Must not depend on: `window`, `editor` or `command`, or file I/O; the host reads the
//! file and hands over what it found.

use std::path::PathBuf;

use super::recovery::NO_TIME;
use super::{Buffer, DiskState, FileStamp, OffsetEdit, Rewrap, StagedEdit};
use crate::decode::FileInfo;
use crate::load::{Change, LoadSnapshot};
use crate::secret::{self, stricter};
use crate::selection::SelectionSet;
use crate::undo::{Opening, Splice};

/// What a look at the file found that the buffer must act on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Noticed {
    /// The file changed under a buffer with no unsaved edits: read it again.
    Reload,
    /// The file changed under unsaved edits, which stay.
    Changed,
    /// The file is gone, or no longer a regular file.
    Gone,
}

/// Whether `seen` is the file `read`, as a buffer read or wrote it: by time and size, and
/// by device and inode where both say.
fn is_as_read(seen: DiskState, read: Option<FileStamp>) -> bool {
    match (seen, read) {
        (DiskState::Gone, None) => true,
        // A copy's text put in for a file gone says so already, and a save makes the file.
        (DiskState::Gone, Some(read)) if read.modified == NO_TIME => true,
        (DiskState::File(stamp), Some(read)) => stamp.same_file(read),
        _ => false,
    }
}

impl Buffer {
    /// Takes in what a look at the file found, and says what must be done about it, once
    /// for each thing seen. A file as read clears what was known of a change, and gives the
    /// baseline its mode, as `chmod` changes no text. A buffer loading, saving, or with no
    /// baseline has nothing to compare with.
    pub(crate) fn file_seen(&mut self, seen: DiskState) -> Option<Noticed> {
        if self.loading.is_some() || self.saving {
            return None;
        }
        let modified = self.is_modified();
        let baseline = self.baseline.as_mut()?;
        if is_as_read(seen, baseline.file) {
            if let (DiskState::File(stamp), Some(read)) = (seen, &mut baseline.file) {
                read.mode = stamp.mode;
            }
            self.disk_acted = None;
            self.gone_from_disk = false;
            self.reload_failed = false;
            self.set_changed_on_disk(false);
            return None;
        }
        let noticed = match seen {
            DiskState::Unknown => return None,
            DiskState::File(_) if !modified => Noticed::Reload,
            DiskState::File(_) => Noticed::Changed,
            DiskState::Gone | DiskState::NotAFile => Noticed::Gone,
        };
        if self.disk_acted == Some((seen, noticed)) {
            return None;
        }
        self.disk_acted = Some((seen, noticed));
        match (noticed, seen) {
            (Noticed::Changed, DiskState::File(stamp)) => {
                self.gone_from_disk = false;
                self.set_changed_on_disk(true);
                // A save asks before it goes over this file, and replaces only this one.
                self.save_over = Some(stamp);
            }
            (Noticed::Gone, _) => self.gone_from_disk = true,
            _ => {}
        }
        Some(noticed)
    }

    /// Whether a save found the file changed since this buffer read it or wrote it.
    pub(crate) fn changed_on_disk(&self) -> bool {
        self.changed_on_disk
    }

    /// Records whether a save found the file changed. A file that has not changed has
    /// nothing to save over.
    pub(crate) fn set_changed_on_disk(&mut self, changed: bool) {
        self.changed_on_disk = changed;
        if !changed {
            self.save_over = None;
        }
    }

    /// The regular file a save found changed on disk, which a save the person agrees to
    /// may replace, if it is still that file (ADR 0072).
    pub(crate) fn save_over(&self) -> Option<FileStamp> {
        self.save_over.filter(|_| self.changed_on_disk)
    }

    pub(crate) fn set_save_over(&mut self, found: FileStamp) {
        self.save_over = Some(found);
    }

    pub(crate) fn forget_save_over(&mut self) {
        self.save_over = None;
    }

    /// Whether the file could not be read again since it changed.
    pub(crate) fn reload_failed(&self) -> bool {
        self.reload_failed
    }

    /// The file could not be read again: it counts as changed on disk, with edits or not.
    pub(crate) fn set_reload_failed(&mut self) {
        self.reload_failed = true;
        self.changed_on_disk = true;
    }

    /// What was done about the file is to be decided again, as a read of it did not go in.
    pub(crate) fn forget_disk_action(&mut self) {
        self.disk_acted = None;
    }

    /// Whether the file was found gone since it was read or written, or no longer a file.
    pub(crate) fn gone_from_disk(&self) -> bool {
        self.gone_from_disk
    }

    pub(crate) fn set_gone_from_disk(&mut self, gone: bool) {
        self.gone_from_disk = gone;
    }

    /// Records the BOM and line ending of the file read again, whose text is the same.
    pub(crate) fn set_file_info(&mut self, info: FileInfo) {
        self.info = info;
    }

    /// Judges the file again, as it was read again: from where its path now leads, or from
    /// what the walk passed when nobody could tell. It never withholds less than before, as
    /// undo can bring back the text that was read under the old verdict.
    pub(crate) fn judge_again(&mut self, resolved: Result<&[PathBuf], &[PathBuf]>) {
        let path = self.path.as_deref();
        let verdict = match resolved {
            Ok(paths) => secret::judge(path, Some(paths)),
            Err(seen) => secret::judge_unresolved(path, seen),
        };
        self.verdict = stricter(self.verdict, verdict);
        self.recheck_highlighting();
    }

    /// The text read again, on clones, as one edit of the bytes `change` covers that undoes
    /// as one step. `None` if either text cannot be edited, as a binary file or invalid UTF-8
    /// cannot: such a text replaces the old one outright.
    pub(crate) fn stage_reload(
        &self,
        snapshot: &LoadSnapshot,
        change: Change,
    ) -> Option<StagedEdit> {
        if self.read_only().is_some()
            || snapshot.info.binary
            || !snapshot.display.invalid_bytes().is_empty()
        {
            return None;
        }
        let Change {
            start,
            old_end,
            new_end,
        } = change;
        let edits = vec![OffsetEdit {
            start,
            old_len: old_end - start,
            new_len: new_end - start,
        }];
        let highlighting =
            self.stage_highlighting(std::slice::from_ref(&edits), snapshot.text.len());
        Some(StagedEdit {
            text: snapshot.text.clone(),
            display: snapshot.display.clone(),
            edits,
            undo: vec![Splice {
                start,
                len: new_end - start,
                text: (old_end > start).then(|| self.text.slice(start..old_end)),
            }],
            // The text was read again whole, with its own display map.
            changed: std::iter::once(0..snapshot.text.len()).collect(),
            rewrap: Rewrap::Whole,
            highlighting,
        })
    }

    /// Puts a staged reload in, as what the file on disk holds now.
    pub(crate) fn commit_reload(
        &mut self,
        staged: StagedEdit,
        snapshot: &LoadSnapshot,
        opening: Opening,
        after: Option<SelectionSet>,
    ) {
        self.commit_edit(staged, opening, after);
        // A report or a job from before a reload is dropped, not mapped (ADR 0111).
        self.highlighting.forget_log();
        self.info = snapshot.info;
        self.saved = self.content;
        self.reloaded();
    }

    /// Puts a text read again in outright, with no undo, for a text that cannot be edited.
    pub(crate) fn replace_with_reload(&mut self, snapshot: LoadSnapshot) {
        self.apply_load(snapshot);
        self.reloaded();
    }

    /// The buffer holds what its file holds now: nothing is changed on disk, nothing is to be
    /// saved over, and a copy's text put in earlier is no longer what undo goes back to, as
    /// its side would bring back what a save compares from before the file changed.
    pub(crate) fn reloaded(&mut self) {
        self.set_changed_on_disk(false);
        self.gone_from_disk = false;
        self.disk_acted = None;
        self.reload_failed = false;
        self.recovery = None;
        self.clear_edited();
    }
}
