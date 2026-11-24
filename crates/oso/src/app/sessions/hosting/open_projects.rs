//! The projects open buffers are in, as the lookups of "an open buffer of the project" count
//! them (ADR 0132): the user's files and terminals, then the native sessions' transcripts,
//! which are internal buffers, so that a native session's project is as open as a TUI
//! session's is by its terminal (ADR 0165).
//!
//! Must not depend on: the disk; the editor's buffers and projects say it.
//!
//! - What judges a native start or resume asks the files and terminals alone, never a
//!   transcript (`trust`, `session_trusts`).
//! - A session's transcript counts while its session is in the table, archived or not: not one whose start is
//!   under way, nor one set aside while its session runs in the TUI, whose terminal counts.

use std::path::Path;

use oso_core::buffer::{BufferId, Mode};
use oso_core::editor::Editor;
use oso_core::project::ProjectId;

use crate::app::App;
use crate::git::TrustedRoot;

/// The projects of the user's buffers, files and terminals, in the order they were added.
pub(super) fn of_files_and_terminals(editor: &Editor) -> impl Iterator<Item = ProjectId> + '_ {
    editor.buffers().filter_map(|(_, buffer)| buffer.project())
}

/// Those and then the projects of `transcripts`.
pub(super) fn of_all(
    editor: &Editor,
    transcripts: impl Iterator<Item = BufferId>,
) -> impl Iterator<Item = ProjectId> {
    of_files_and_terminals(editor).chain(of_transcripts(editor, transcripts))
}

/// The projects of those of `buffers` that are there and are native sessions' transcripts.
pub(super) fn of_transcripts(
    editor: &Editor,
    buffers: impl Iterator<Item = BufferId>,
) -> impl Iterator<Item = ProjectId> {
    buffers
        .filter_map(|id| editor.buffer(id))
        .filter(|buffer| buffer.is_native_session())
        .filter_map(|buffer| buffer.project())
}

/// The first of `open` whose root is `root`.
pub(super) fn at_root(
    editor: &Editor,
    mut open: impl Iterator<Item = ProjectId>,
    root: &Path,
) -> Option<ProjectId> {
    open.find(|&id| editor.project(id).is_some_and(|p| p.root() == root))
}

/// The trust of the project at `root` as one of `open` has it now: `None` when none is there,
/// `Some(None)` when it is not trusted.
pub(super) fn trust_at(
    editor: &Editor,
    open: impl Iterator<Item = ProjectId>,
    root: &Path,
) -> Option<Option<TrustedRoot>> {
    let id = at_root(editor, open, root)?;
    Some(TrustedRoot::of(editor, id))
}

impl App {
    /// The projects open buffers are in, files' and terminals' first, then the native sessions'
    /// transcripts', each as often as a buffer is in it.
    pub(in crate::app) fn open_projects(&self) -> impl Iterator<Item = ProjectId> + '_ {
        of_all(&self.editor, self.transcripts())
    }

    /// The projects of the native sessions' transcripts.
    pub(super) fn transcript_projects(&self) -> impl Iterator<Item = ProjectId> + '_ {
        of_transcripts(&self.editor, self.transcripts())
    }

    /// The buffers of the native sessions' transcripts, in the table's order.
    fn transcripts(&self) -> impl Iterator<Item = BufferId> + '_ {
        let natives = &self.hosting.natives;
        (self.hosting.table.iter()).filter_map(|session| natives.transcript(session.key))
    }

    /// The first open project whose root is `root`.
    pub(in crate::app) fn open_project_at(&self, root: &Path) -> Option<ProjectId> {
        at_root(&self.editor, self.open_projects(), root)
    }

    /// The trust of the project at `root` as an open buffer of it has it now, a transcript
    /// among them, for the session tools: `None` when no buffer has it open, `Some(None)` when
    /// it is not trusted. Never for whether a native session may start or resume.
    pub(in crate::app) fn open_trust_at(&self, root: &Path) -> Option<Option<TrustedRoot>> {
        trust_at(&self.editor, self.open_projects(), root)
    }

    /// The project whose root `directory` is: the selected buffer's, or in `*Spaces*` any open
    /// buffer's, as no buffer there is in one.
    pub(super) fn project_at_root(&self, directory: &Path) -> Option<ProjectId> {
        let current = self.editor.current().1;
        at_root(&self.editor, current.project().into_iter(), directory).or_else(|| {
            (current.mode() == Mode::Sessions)
                .then(|| self.open_project_at(directory))
                .flatten()
        })
    }
}

#[cfg(test)]
pub(super) mod tests;
