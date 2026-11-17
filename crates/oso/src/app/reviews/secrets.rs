//! What of a review may hold secrets (ADR 0122, "Secrets and display"), one rule for the
//! message sent to a session (step 12) and the review saved to disk (step 6): a file whose name
//! may hold secrets, as the git status judges it (ADR 0127), or whose open buffer may hold
//! secrets or is withheld from agents.
//!
//! Must not depend on: key presses, threads or how a frame is drawn; the UI thread gathers the
//! buffers, and a path is judged wherever it is asked.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use oso_core::editor::Editor;

/// The files of the buffers that may hold secrets or are withheld from agents.
pub(super) fn held_paths(editor: &Editor) -> HashSet<PathBuf> {
    (editor.buffers())
        .filter(|(_, buffer)| buffer.is_secret() || buffer.verdict().is_withheld())
        .filter_map(|(_, buffer)| buffer.path().map(PathBuf::from))
        .collect()
}

/// Whether the file at `path`, as git gives it under `root`, may hold secrets: by its name, or
/// as `held`, the files of such buffers, holds it.
pub(super) fn withheld(root: &Path, held: &HashSet<PathBuf>, path: &[u8]) -> bool {
    crate::git::may_hold_secrets(root, path)
        || crate::git::os_path(path).is_ok_and(|path| held.contains(&root.join(path)))
}
