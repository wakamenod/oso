//! Internal buffers: text the editor keeps for itself, such as `*Messages*`
//! (ADR 0051, ADR 0063).
//!
//! Must not depend on: `window`, `editor` or `command`.
//!
//! An internal buffer visits no file, a key cannot edit it, and it holds nothing
//! that could be lost. So it is never modified, and only the editor changes it,
//! with no undo history.

use super::{Buffer, Replacement, StagedEdit, normalized};

impl Buffer {
    /// An empty internal buffer.
    pub(crate) fn internal(name: impl Into<String>) -> Self {
        Self {
            internal: true,
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
    }
}
