//! Which internal buffers are listed and killed as a person's buffer is (ADR 0063): the rule
//! `C-x b`, `C-x C-b` and `C-x k` share.
//!
//! Must not depend on: the host's window; a plain function of a buffer.

use crate::buffer::{Buffer, Mode};

/// Whether `buffer`, though internal, is listed and killed as a person's buffer is: a
/// directory's listing (ADR 0134), a git status, log, graph or pull request (ADR 0127), as
/// Emacs's dired and magit buffers are, a rename's preview (ADR 0138), what a session's move
/// waits for, whose kill cancels it (ADR 0151), or a native session's (ADR 0165).
pub(in crate::editor) fn kept_as_users(buffer: &Buffer) -> bool {
    let kept = [Mode::GitDiscarded, Mode::RenamePreview, Mode::SessionMove];
    super::super::listings::is_listing(buffer)
        || super::super::git::is_git_status(buffer)
        || super::super::git::is_git_log(buffer)
        || super::super::git::is_git_graph(buffer)
        || super::super::git::is_git_pr(buffer)
        || super::super::git::is_git_issue(buffer)
        || (buffer.is_internal() && kept.contains(&buffer.mode()))
        || buffer.is_native_session()
}
