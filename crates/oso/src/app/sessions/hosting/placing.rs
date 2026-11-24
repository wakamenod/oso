//! The project a conversation is placed by, adopted or forked where no session of oso's ran it
//! (ADR 0121, ADR 0145): the deepest open one that holds its directory.
//!
//! Must not depend on: the disk; the editor's buffers and projects say it.
//!
//! - A native session's transcript counts as open (ADR 0165), but never changes whether the
//!   conversation runs natively, and a restricted one never takes it out of the project of an
//!   open file or terminal.

use std::collections::HashSet;
use std::path::Path;

use oso_core::editor::Editor;
use oso_core::project::ProjectId;

use super::open_projects::of_files_and_terminals;
use crate::app::App;
use crate::sessions::adopt;

/// The deepest of `open` whose root holds `directory`.
fn holding(
    editor: &Editor,
    open: impl Iterator<Item = ProjectId>,
    directory: &Path,
) -> Option<ProjectId> {
    let mut seen = HashSet::new();
    let roots = open
        .filter(|id| seen.insert(*id))
        .filter_map(|id| Some((id, editor.project(id)?.root())));
    adopt::holding(directory, roots)
}

/// The deepest of the files' and terminals' projects and `transcripts` that holds `directory`;
/// `native` says whether the conversation runs natively placed at a root, or at none.
fn placing(
    editor: &Editor,
    transcripts: impl Iterator<Item = ProjectId>,
    directory: &Path,
    native: impl Fn(Option<&Path>) -> bool,
) -> Option<ProjectId> {
    let by_file = holding(editor, of_files_and_terminals(editor), directory);
    let transcripts = transcripts.filter(|&id| by_file.is_none() || editor.project_trusted(id));
    let either = holding(editor, by_file.into_iter().chain(transcripts), directory);
    let root = |id: Option<ProjectId>| Some(editor.project(id?)?.root());
    if root(either) != root(by_file) && native(root(either)) == native(root(by_file)) {
        either
    } else {
        by_file
    }
}

impl App {
    /// The deepest open project whose root holds `directory`, which places a conversation
    /// there.
    pub(super) fn project_holding(&self, directory: &Path) -> Option<ProjectId> {
        placing(
            &self.editor,
            self.transcript_projects(),
            directory,
            |root| self.native_trust_listed(directory, root, false).is_ok(),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use oso_core::buffer::BufferId;
    use oso_core::editor::Editor;
    use oso_core::project::TrustList;

    use super::super::open_projects::of_transcripts;
    use super::super::open_projects::tests::{file, record, transcript};
    use super::placing;

    /// The root a conversation in `/w/r/in` is placed by, native where `native` says.
    fn placed(
        editor: &Editor,
        ids: &[BufferId],
        native: impl Fn(Option<&Path>) -> bool,
    ) -> PathBuf {
        let transcripts = of_transcripts(editor, ids.iter().copied());
        placing(editor, transcripts, Path::new("/w/r/in"), native)
            .and_then(|id| editor.project(id))
            .map(|project| project.root().to_owned())
            .unwrap_or_default()
    }

    #[test]
    fn a_conversation_is_placed_by_the_deepest_open_project_a_transcripts_among_them() {
        let mut editor = Editor::default();
        editor.set_trust_list(Some(record("/w/r/in")));
        let never = |_: Option<&Path>| false;
        assert_eq!(placed(&editor, &[], never), PathBuf::new());
        let outer = transcript(&mut editor, "/w/r");
        assert_eq!(placed(&editor, &[outer], never), Path::new("/w/r"));
        let own = transcript(&mut editor, "/w/r/in");
        assert_eq!(placed(&editor, &[outer, own], never), Path::new("/w/r/in"));
        file(&mut editor, "/w/r");
        assert_eq!(placed(&editor, &[outer], never), Path::new("/w/r"));
        assert_eq!(placed(&editor, &[outer, own], never), Path::new("/w/r/in"));
        editor.set_trust_list(Some(TrustList::default()));
        assert_eq!(
            placed(&editor, &[own], never),
            Path::new("/w/r"),
            "restricted"
        );
        let deeper = transcript(&mut editor, "/w/r/in/deeper");
        assert_eq!(placed(&editor, &[deeper], never), Path::new("/w/r"));
    }

    /// A nested root the record itself holds runs natively at its own root and nowhere else, so
    /// no transcript moves a conversation there or away from there.
    #[test]
    fn a_transcript_never_changes_whether_a_conversation_runs_natively() {
        let at_own =
            |root: Option<&Path>| root.unwrap_or(Path::new("/w/r/in")) == Path::new("/w/r/in");
        // With no file open it runs at its directory, natively; the outer transcript does not count.
        let mut editor = Editor::default();
        editor.set_trust_list(Some(record("/w/r/in")));
        let outer = transcript(&mut editor, "/w/r");
        let own = transcript(&mut editor, "/w/r/in");
        assert_eq!(placed(&editor, &[outer], at_own), PathBuf::new());
        // Its own root's transcript leaves it there.
        assert_eq!(placed(&editor, &[outer, own], at_own), Path::new("/w/r/in"));
        // With a file of the outer project open it does not, and its own root's does not count.
        file(&mut editor, "/w/r");
        assert_eq!(placed(&editor, &[outer, own], at_own), Path::new("/w/r"));
    }
}
