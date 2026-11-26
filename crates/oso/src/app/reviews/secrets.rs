//! What of a review may hold secrets (ADR 0122, "Secrets and display"), one rule for the
//! message sent to a session (step 12) and the review saved to disk (step 6): a file whose name
//! may hold secrets, as the git status judges it (ADR 0127), or whose open buffer may hold
//! secrets or is withheld from agents.
//!
//! Must not depend on: key presses, threads or how a frame is drawn; the UI thread gathers the
//! buffers, and a path is judged wherever it is asked. Each view is told those buffers' files,
//! which show no unchanged line around their hunks (step 13).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use std::sync::Arc;

use oso_core::buffer::Verdict;
use oso_core::editor::Editor;

use crate::app::App;
use crate::review::view::Tell;

/// The files of the buffers that may hold secrets or are withheld from agents.
pub(super) fn held_paths(editor: &Editor) -> HashSet<PathBuf> {
    (editor.buffers())
        .filter(|(_, buffer)| buffer.is_secret() || buffer.verdict().is_withheld())
        .filter_map(|(_, buffer)| buffer.path().map(PathBuf::from))
        .collect()
}

/// The files held as secret while a review is open (ADR 0184): `sticky`, those of buffers ever
/// judged to hold secrets or whose path could not be resolved, never let go in this run; and
/// `unjudged`, those of buffers not judged yet now, held only while they are not. Notes the
/// buffers now, and returns whether either changed.
pub(super) fn note_held(
    editor: &Editor,
    sticky: &mut HashSet<PathBuf>,
    unjudged: &mut HashSet<PathBuf>,
) -> bool {
    let mut now = HashSet::new();
    let mut grew = false;
    for (_, buffer) in editor.buffers() {
        let Some(path) = buffer.path() else {
            continue;
        };
        let verdict = buffer.verdict();
        if verdict == Verdict::Unjudged {
            now.insert(path.to_path_buf());
        } else if verdict.is_withheld() && !sticky.contains(path) {
            sticky.insert(path.to_path_buf());
            grew = true;
        }
    }
    let changed = grew || now != *unjudged;
    *unjudged = now;
    changed
}

/// Whether the file at `path`, as git gives it under `root`, may hold secrets: by its name, or
/// as `held`, the files of such buffers, holds it.
pub(super) fn withheld(root: &Path, held: &HashSet<PathBuf>, path: &[u8]) -> bool {
    crate::git::may_hold_secrets(root, path)
        || crate::git::os_path(path).is_ok_and(|path| held.contains(&root.join(path)))
}

impl App {
    /// After commands and each event: tells each review's view the files of the open buffers
    /// that may hold secrets, when they changed. Costs a look at each buffer, and nothing with
    /// no review open.
    pub(super) fn sync_review_held(&mut self) {
        if self.reviews.open.is_empty() {
            return;
        }
        let held = held_paths(&self.editor);
        for running in &mut self.reviews.open {
            if *running.held != held {
                running.held = Arc::new(held.clone());
                running
                    .view
                    .tell(Tell::HeldBuffers(Arc::clone(&running.held)));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::PathBuf;

    use oso_core::buffer::{Buffer, BufferId};
    use oso_core::editor::Editor;

    use super::note_held;

    /// The held files, sticky and not judged yet, as a review's saving notes them each event.
    #[derive(Default)]
    struct Held {
        sticky: HashSet<PathBuf>,
        unjudged: HashSet<PathBuf>,
    }

    impl Held {
        /// Notes `editor`'s buffers, and gives the files held: the two sets' union.
        fn note(&mut self, editor: &Editor) -> HashSet<PathBuf> {
            note_held(editor, &mut self.sticky, &mut self.unjudged);
            self.sticky.union(&self.unjudged).cloned().collect()
        }
    }

    /// A buffer visiting `path`, not judged yet, as one loading is, and as a restart's carried
    /// buffer is while it is read again.
    fn visiting(editor: &mut Editor, path: &str) -> BufferId {
        editor.add_buffer(Buffer::visiting(PathBuf::from(path)))
    }

    /// Judges `id` as a load does, with `path` where it leads, or none for a path not resolved.
    fn judge(editor: &mut Editor, id: BufferId, path: Option<&str>) {
        let resolved = path.map(|path| vec![PathBuf::from(path)]);
        (editor.buffer_mut(id).unwrap()).judge_secret(resolved.as_deref());
    }

    fn set(paths: &[&str]) -> HashSet<PathBuf> {
        paths.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn a_file_not_judged_yet_is_held_while_it_is_not() {
        let mut editor = Editor::default();
        let mut held = Held::default();
        let readme = visiting(&mut editor, "/src/repo/README.md");
        assert_eq!(held.note(&editor), set(&["/src/repo/README.md"]));
        // Judged to hold no secret: let go.
        judge(&mut editor, readme, Some("/src/repo/README.md"));
        assert_eq!(held.note(&editor), set(&[]));
    }

    #[test]
    fn a_file_judged_to_hold_secrets_stays_held_for_the_run() {
        let mut editor = Editor::default();
        let mut held = Held::default();
        let env = visiting(&mut editor, "/src/repo/.env");
        judge(&mut editor, env, Some("/src/repo/.env"));
        assert_eq!(held.note(&editor), set(&["/src/repo/.env"]));
        // Its buffer gone, or the name it leads to judged again: still held.
        editor.kill_buffer(env);
        assert_eq!(held.note(&editor), set(&["/src/repo/.env"]));
        let again = visiting(&mut editor, "/src/repo/.env");
        assert_eq!(held.note(&editor), set(&["/src/repo/.env"]));
        judge(&mut editor, again, Some("/src/repo/plain.txt"));
        assert_eq!(held.note(&editor), set(&["/src/repo/.env"]));
    }

    #[test]
    fn a_file_whose_path_cannot_be_resolved_stays_held_for_the_run() {
        let mut editor = Editor::default();
        let mut held = Held::default();
        let lost = visiting(&mut editor, "/src/repo/lost.rs");
        judge(&mut editor, lost, None);
        assert_eq!(held.note(&editor), set(&["/src/repo/lost.rs"]));
        editor.kill_buffer(lost);
        assert_eq!(held.note(&editor), set(&["/src/repo/lost.rs"]));
    }

    #[test]
    fn a_file_not_judged_then_judged_to_hold_secrets_moves_to_the_held_for_the_run() {
        let mut editor = Editor::default();
        let mut held = Held::default();
        let key = visiting(&mut editor, "/src/repo/link.txt");
        assert!(note_held(&editor, &mut held.sticky, &mut held.unjudged));
        assert_eq!(held.unjudged, set(&["/src/repo/link.txt"]));
        // It leads to a key: held for the run, and no longer as one not judged.
        judge(&mut editor, key, Some("/home/me/.ssh/id_rsa"));
        assert!(note_held(&editor, &mut held.sticky, &mut held.unjudged));
        assert_eq!(
            (held.sticky.clone(), held.unjudged.clone()),
            (set(&["/src/repo/link.txt"]), set(&[]))
        );
        // Nothing changed since: says so.
        assert!(!note_held(&editor, &mut held.sticky, &mut held.unjudged));
    }
}
