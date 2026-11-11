//! Internal buffers: text the editor keeps for itself, such as `*Messages*`
//! (ADR 0051, ADR 0063).
//!
//! Must not depend on: `window`, `editor` or `command`.
//!
//! An internal buffer visits no file and holds nothing that could be lost, so it is
//! never handed over, saved or counted at an exit. Most are read-only to keys, and
//! only the editor changes them, with no undo history. A prompt's buffer takes typing
//! and undo like any other, and goes when its prompt closes (ADR 0072).

use super::{Buffer, Replacement, StagedEdit, normalized};
use crate::secret::Verdict;

impl Buffer {
    /// An empty internal buffer.
    pub(crate) fn internal(name: impl Into<String>) -> Self {
        Self {
            internal: true,
            ..Self::new(name)
        }
    }

    /// An empty internal buffer for a prompt, which takes typing. One that reads a
    /// passphrase may hold secrets and keeps no undo record (ADR 0072).
    pub(crate) fn prompt(name: impl Into<String>, passphrase: bool) -> Self {
        Self {
            internal: true,
            takes_typing: true,
            no_history: passphrase,
            verdict: if passphrase {
                Verdict::Passphrase
            } else {
                Verdict::Ordinary
            },
            ..Self::new(name)
        }
    }

    /// Whether the editor keeps this buffer for itself. It is not the user's work:
    /// it is not handed over, saved or counted at an exit.
    pub fn is_internal(&self) -> bool {
        self.internal
    }

    /// Makes `edits` on clones, whatever [`Buffer::read_only`] says. `None` if all
    /// are empty.
    pub(crate) fn stage_internal(&self, edits: &[Replacement<'_>]) -> Option<StagedEdit> {
        let splices = normalized(&self.text, edits);
        (!splices.is_empty()).then(|| self.stage_splices(&splices))
    }

    /// Puts a staged edit in the buffer with no undo record, and leaves it
    /// unmodified. Nothing here can panic part way.
    pub(crate) fn commit_internal(&mut self, staged: StagedEdit) {
        self.text = staged.text;
        self.display = staged.display;
        self.revision += 1;
        self.highlighting.commit(self.revision, staged.highlighting);
    }
}
