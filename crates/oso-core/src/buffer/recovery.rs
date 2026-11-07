//! What a buffer knows of emergency copies (ADR 0064): the modes of the buffers that show
//! them, whether one waits for an answer, and a whole text put in from one.
//!
//! Must not depend on: `window`, `editor` or `command`, or file I/O; the host reads the
//! copies and hands over what it found.

use std::path::PathBuf;

use super::{Baseline, Buffer, ContentId, EditError, FileStamp, OffsetEdit, StagedEdit, Verdict};
use crate::decode::LineEnding;
use crate::load::Progress;
use crate::secret;
use crate::selection::SelectionSet;
use crate::undo::{Opening, Splice};

/// Which keys a buffer takes and what its modeline calls it, until major modes arrive (M3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Mode {
    /// A buffer of the user's, or `*Messages*`.
    #[default]
    Fundamental,
    /// The list of emergency copies.
    EmergencyCopies,
    /// The text of one emergency copy, or how it differs from its file.
    EmergencyCopy,
}

impl Mode {
    /// What the modeline calls it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Fundamental => "Fundamental",
            Self::EmergencyCopies => "Emergency Copies",
            Self::EmergencyCopy => "Emergency Copy",
        }
    }

    /// Whether this is one of the buffers that show emergency copies.
    pub fn shows_copies(self) -> bool {
        self != Self::Fundamental
    }
}

/// A copy's text was put in this buffer, as the states on either side of it say.
#[derive(Clone, Copy, Debug)]
pub(super) struct RecoveryMark {
    before: ContentId,
    after: ContentId,
    /// False while undo has gone back to `before`.
    stands: bool,
}

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
    /// nothing touches the disk. With no path to go on, it counts as not known (ADR 0064).
    pub fn judge_copy(&mut self, paths: &[PathBuf], at_least: Verdict) {
        self.verdict = secret::judge_copy(paths, at_least);
    }

    /// Sets the BOM and line ending a save writes, from a copy, when no file says.
    pub fn set_file_format(&mut self, bom: bool, line_ending: LineEnding) {
        self.info.bom = bom;
        self.info.line_ending = line_ending;
    }

    /// Records that the file has gone and comes back as `0600`: a save takes that for a file
    /// deleted since it was read (ADR 0059), and a private file must not come back as what
    /// the umask gives. Does nothing with no location to go by.
    pub fn expect_a_file_that_has_gone(&mut self, stamp: FileStamp) {
        if let Some(baseline) = &self.baseline {
            self.baseline = Some(Baseline {
                location: baseline.location.clone(),
                file: Some(stamp),
            });
        }
    }

    /// The whole text of `source` on clones, in O(1), as one edit that undoes as one step.
    pub(crate) fn stage_replacement(&self, source: &Buffer) -> Result<StagedEdit, EditError> {
        if let Some(reason) = self.read_only() {
            return Err(EditError::ReadOnly(reason));
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

    /// Puts a whole text in, and remembers the states on either side of it.
    pub(crate) fn commit_recovery(
        &mut self,
        staged: StagedEdit,
        opening: Opening,
        after: Option<SelectionSet>,
    ) {
        let before = self.content;
        self.commit_edit(staged, opening, after);
        self.recovery = Some(RecoveryMark {
            before,
            after: self.content,
            stands: true,
        });
    }

    /// An undo or a redo has arrived at the state now: going back past a recovery withdraws
    /// it, and redoing it brings it back.
    pub(super) fn note_crossing(&mut self) {
        if let Some(mark) = &mut self.recovery {
            if self.content == mark.before {
                mark.stands = false;
            } else if self.content == mark.after {
                mark.stands = true;
            }
        }
    }

    /// Whether a copy's text was put in this buffer, and no undo has taken that back.
    pub fn recovery_stands(&self) -> bool {
        self.recovery.is_some_and(|mark| mark.stands)
    }

    /// The recovery has been dealt with: its copy has gone.
    pub fn forget_recovery(&mut self) {
        self.recovery = None;
    }
}
