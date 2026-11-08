//! A buffer's file read again after it changed on disk (ADR 0029, ADR 0083): what is known of
//! the file's state, the verdict judged again, and the new text put in as one undo step.
//!
//! Must not depend on: `window`, `editor` or `command`, or file I/O; the host reads the
//! file and hands over what it found.

use std::path::PathBuf;

use super::{Buffer, OffsetEdit, StagedEdit};
use crate::decode::FileInfo;
use crate::load::{Change, LoadSnapshot};
use crate::secret::{self, stricter};
use crate::selection::SelectionSet;
use crate::undo::{Opening, Splice};

impl Buffer {
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
        Some(StagedEdit {
            text: snapshot.text.clone(),
            display: snapshot.display.clone(),
            edits: vec![OffsetEdit {
                start,
                old_len: old_end - start,
                new_len: new_end - start,
            }],
            undo: vec![Splice {
                start,
                len: new_end - start,
                text: (old_end > start).then(|| self.text.slice(start..old_end)),
            }],
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
        self.recovery = None;
    }
}
