//! What a buffer knows of emergency copies (ADR 0064): the modes of the buffers that show
//! them, whether one waits for an answer, and a whole text put in from one.
//!
//! Must not depend on: `window`, `editor` or `command`, or file I/O; the host reads the
//! copies and hands over what it found.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::{
    Baseline, Buffer, ContentId, EditError, FileStamp, Mode, OffsetEdit, ReadOnly, StagedEdit,
    Verdict,
};
use crate::decode::{FileInfo, LineEnding};
use crate::load::Progress;
use crate::secret::{self, stricter};
use crate::selection::SelectionSet;
use crate::undo::{Opening, Splice};

/// A copy's text was put in this buffer, as the states on either side of it say.
#[derive(Clone, Debug)]
pub(super) struct RecoveryMark {
    before: ContentId,
    after: ContentId,
    /// False while undo has gone back to `before`.
    stands: bool,
    /// What goes with the text on each side, as undo must take these back too.
    side_before: Side,
    side_after: Side,
}

/// What goes with a text: what a save writes and expects, and whether it may hold secrets.
#[derive(Clone, Debug)]
struct Side {
    info: FileInfo,
    baseline: Option<Baseline>,
    verdict: Verdict,
}

/// The modification time of a file that has gone, by which no file can pass for it.
pub(super) const NO_TIME: SystemTime = SystemTime::UNIX_EPOCH;

impl Buffer {
    /// Which keys this buffer takes.
    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub(crate) fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
    }

    /// Fills this buffer from a thread: it reads as loading until the last snapshot.
    pub fn begin_loading(&mut self) {
        self.loading = Some(Progress::default());
    }

    /// Records the file's own spelling, where a file system that ignores case or normalizes
    /// names opens it under others. An emergency copy keeps this, so that the next run finds it
    /// however the file is typed. It is not what a save compares (ADR 0064).
    pub fn set_own_spelling(&mut self, spelling: PathBuf) {
        self.own_spelling = Some(spelling);
    }

    /// The place to record of the file in an emergency copy: its own spelling if that could be
    /// told, or else where the path led.
    pub fn place_for_copy(&self) -> Option<&Path> {
        self.own_spelling.as_deref().or(self
            .baseline
            .as_ref()
            .map(|baseline| baseline.location.as_path()))
    }

    /// Whether an emergency copy of this buffer's file waits for an answer.
    pub(crate) fn copy_waits(&self) -> bool {
        self.copy_waits
    }

    /// Says whether an emergency copy of this buffer's file waits for an answer.
    pub fn set_copy_waits(&mut self, waits: bool) {
        self.copy_waits = waits;
    }

    /// Decides whether an internal buffer holding an emergency copy may hold secrets, from
    /// the paths its metadata names, and at least `at_least`. These are only compared, so
    /// nothing touches the disk (ADR 0064).
    pub fn judge_copy(&mut self, paths: &[PathBuf], at_least: Verdict) {
        self.verdict = secret::judge_copy(paths, at_least);
    }

    /// Sets the BOM and line ending a save writes, from a copy, when no file says. If
    /// `file_has_gone`, the file comes back as `0600` too: a save takes that for a file deleted
    /// since it was read (ADR 0059), and a private file must not come back as what the umask
    /// gives. Nothing from the copy reaches what a save compares. An undo of the recovery takes
    /// all of this back.
    pub fn take_format_of_copy(&mut self, bom: bool, line_ending: LineEnding, file_has_gone: bool) {
        self.info.bom = bom;
        self.info.line_ending = line_ending;
        if file_has_gone && let Some(baseline) = &mut self.baseline {
            baseline.file = Some(FileStamp {
                modified: NO_TIME,
                len: 0,
                mode: 0o600,
                identity: None,
            });
        }
        let side = self.side();
        if let Some(mark) = &mut self.recovery {
            mark.side_after = side;
        }
    }

    /// The whole text of `source` on clones, in O(1), as one edit that undoes as one step.
    pub(crate) fn stage_replacement(&self, source: &Buffer) -> Result<StagedEdit, EditError> {
        if let Some(reason) = self.read_only() {
            return Err(EditError::ReadOnly(reason));
        }
        if source.loading.is_some() {
            return Err(EditError::ReadOnly(ReadOnly::Loading));
        }
        let (old_len, new_len) = (self.text.len(), source.text.len());
        Ok(StagedEdit {
            text: source.text.clone(),
            display: source.display.clone(),
            edits: vec![OffsetEdit {
                start: 0,
                old_len,
                new_len,
            }],
            undo: vec![Splice {
                start: 0,
                len: new_len,
                text: (old_len > 0).then(|| self.text.clone()),
            }],
        })
    }

    fn side(&self) -> Side {
        Side {
            info: self.info,
            baseline: self.baseline.clone(),
            verdict: self.verdict,
        }
    }

    /// Puts a whole text in, and remembers the states on either side of it.
    pub(crate) fn commit_recovery(
        &mut self,
        staged: StagedEdit,
        opening: Opening,
        after: Option<SelectionSet>,
        source_verdict: Verdict,
    ) {
        let before = self.content;
        let side_before = self.side();
        self.commit_edit(staged, opening, after);
        // The text may be what a view withheld, and it is no less so here.
        self.verdict = stricter(self.verdict, source_verdict);
        self.recovery = Some(RecoveryMark {
            before,
            after: self.content,
            stands: true,
            side_before,
            side_after: self.side(),
        });
    }

    /// A save has put `stamp` in place. Whichever side an undo or a redo goes to, that is what
    /// is there now, as what a save compares must not go back to before it.
    pub(super) fn note_written(&mut self, stamp: FileStamp) {
        if let Some(mark) = &mut self.recovery {
            for side in [&mut mark.side_before, &mut mark.side_after] {
                if let Some(baseline) = &mut side.baseline {
                    baseline.file = Some(stamp);
                }
            }
        }
    }

    /// An undo or a redo has arrived at the state now: going back past a recovery withdraws
    /// it, and redoing it brings it back.
    pub(super) fn note_crossing(&mut self) {
        let Some(mark) = &mut self.recovery else {
            return;
        };
        let side = if self.content == mark.before {
            mark.stands = false;
            mark.side_before.clone()
        } else if self.content == mark.after {
            mark.stands = true;
            mark.side_after.clone()
        } else {
            return;
        };
        self.info = side.info;
        self.baseline = side.baseline;
        self.verdict = side.verdict;
    }

    /// Whether a copy's text was put in this buffer, and no undo has taken that back.
    pub fn recovery_stands(&self) -> bool {
        self.recovery.as_ref().is_some_and(|mark| mark.stands)
    }

    /// The recovery has been dealt with: its copy has gone.
    pub fn forget_recovery(&mut self) {
        self.recovery = None;
    }
}
