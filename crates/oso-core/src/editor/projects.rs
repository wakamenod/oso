//! The projects buffers are in (ADR 0114): the files whose project the host should find, and
//! a table of what it found, which buffers point into.
//!
//! Must not depend on: the host's threads or I/O. The host finds a project on a thread of its
//! own, after the file's text has come, and hands over what it found.

use std::path::PathBuf;

use super::Editor;
use crate::buffer::BufferId;
use crate::project::{Found, Project, ProjectId};

/// A file whose project the host should find: the buffer, and the real place its path leads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectRequest {
    pub buffer: BufferId,
    pub location: PathBuf,
}

impl Editor {
    /// Asks the host to find the project of `id`'s file, once where its path leads is known.
    /// A buffer with no such place is in no project.
    pub fn request_project(&mut self, id: BufferId) {
        let Some(buffer) = self.buffer(id) else {
            return;
        };
        let Some(baseline) = buffer.baseline() else {
            return;
        };
        let request = ProjectRequest {
            buffer: id,
            location: baseline.location.clone(),
        };
        self.project_requests.push(request);
    }

    /// The files whose project the host should find, the oldest first.
    pub fn take_project_requests(&mut self) -> Vec<ProjectRequest> {
        std::mem::take(&mut self.project_requests)
    }

    /// The host found `request`'s file in the project `found`, or in none. It is taken only if
    /// the buffer's path still leads where it did when asked.
    pub fn project_found(&mut self, request: &ProjectRequest, found: Option<Found>) {
        let current = self
            .buffer(request.buffer)
            .and_then(|buffer| buffer.baseline())
            .is_some_and(|baseline| baseline.location == request.location);
        if current {
            self.set_buffer_project(request.buffer, found);
        }
    }

    /// The project `id` names.
    pub fn project(&self, id: ProjectId) -> Option<&Project> {
        self.projects.get(usize::try_from(id.0).ok()?)
    }

    /// The project of the buffer `id`, if its file is in one.
    pub(crate) fn buffer_project(&self, id: BufferId) -> Option<&Project> {
        self.project(self.buffer(id)?.project()?)
    }

    /// Puts `id` in the project `found`, or in none. A project already in the table, by its
    /// root and the directory's identity, is shared, and takes what its `.git` file names this
    /// time; any other is added. So one root is one project.
    pub(crate) fn set_buffer_project(&mut self, id: BufferId, found: Option<Found>) {
        if self.buffer(id).is_none() {
            return;
        }
        let project = found.map(|found| self.intern_project(found));
        if let Some(buffer) = self.buffer_mut(id) {
            buffer.set_project(project);
        }
    }

    fn intern_project(&mut self, found: Found) -> ProjectId {
        if let Some(place) = self
            .projects
            .iter()
            .position(|known| known.found().root == found.root)
        {
            self.projects[place].set_worktree_of(found.worktree_of);
            return ProjectId(u32::try_from(place).unwrap_or(u32::MAX));
        }
        self.projects.push(Project::new(found));
        ProjectId(u32::try_from(self.projects.len() - 1).unwrap_or(u32::MAX))
    }
}

#[cfg(test)]
mod tests {
    use super::ProjectRequest;
    use crate::buffer::{Baseline, Buffer};
    use crate::editor::Editor;
    use crate::project::{Found, RootIdentity};
    use std::path::PathBuf;

    fn found(path: &str, inode: u64) -> Found {
        Found {
            root: RootIdentity {
                path: PathBuf::from(path),
                identity: Some((1, inode)),
            },
            worktree_of: None,
        }
    }

    /// A buffer visiting `path`, whose path was found to lead there.
    fn visited(path: &str) -> Buffer {
        let mut buffer = Buffer::visiting(PathBuf::from(path));
        buffer.set_baseline(Baseline {
            location: PathBuf::from(path),
            file: None,
        });
        buffer
    }

    fn asked(editor: &mut Editor, id: crate::buffer::BufferId) -> ProjectRequest {
        editor.request_project(id);
        editor.take_project_requests().pop().unwrap()
    }

    #[test]
    fn a_request_carries_where_the_path_leads_and_none_comes_without_it() {
        let mut editor = Editor::with_buffer(visited("/w/oso/a.rs"));
        let a = editor.tiled().0.buffer();
        let request = asked(&mut editor, a);
        assert_eq!(request.location, PathBuf::from("/w/oso/a.rs"));
        let loose = editor.add_buffer(Buffer::visiting(PathBuf::from("/w/b.rs")));
        editor.request_project(loose);
        assert!(editor.take_project_requests().is_empty());
    }

    #[test]
    fn buffers_in_one_project_share_it_and_others_get_their_own() {
        let mut editor = Editor::with_buffer(visited("/w/oso/a.rs"));
        let a = editor.tiled().0.buffer();
        let b = editor.add_buffer(visited("/w/oso/b.rs"));
        let c = editor.add_buffer(visited("/w/other/c.rs"));
        for (id, root, inode) in [(a, "/w/oso", 7), (b, "/w/oso", 7), (c, "/w/other", 8)] {
            let request = asked(&mut editor, id);
            editor.project_found(&request, Some(found(root, inode)));
        }
        let project = |id| editor.buffer(id).unwrap().project().unwrap();
        assert_eq!(project(a), project(b));
        assert_ne!(project(a), project(c));
        assert_eq!(editor.buffer_project(a).unwrap().name(), "oso");
        assert_eq!(editor.buffer_project(c).unwrap().name(), "other");
    }

    #[test]
    fn a_root_made_again_at_the_same_path_is_another_project() {
        let mut editor = Editor::with_buffer(visited("/w/oso/a.rs"));
        let a = editor.tiled().0.buffer();
        let b = editor.add_buffer(visited("/w/oso/b.rs"));
        editor.set_buffer_project(a, Some(found("/w/oso", 7)));
        editor.set_buffer_project(b, Some(found("/w/oso", 9)));
        assert_ne!(
            editor.buffer(a).unwrap().project(),
            editor.buffer(b).unwrap().project()
        );
    }

    #[test]
    fn one_root_is_one_project_whatever_its_git_file_named() {
        let mut editor = Editor::with_buffer(visited("/w/wt/a.rs"));
        let a = editor.tiled().0.buffer();
        let b = editor.add_buffer(visited("/w/wt/b.rs"));
        let mut claims = found("/w/wt", 7);
        claims.worktree_of = Some(PathBuf::from("/w/main"));
        editor.set_buffer_project(a, Some(claims));
        editor.set_buffer_project(b, Some(found("/w/wt", 7)));
        let project = editor.buffer(a).unwrap().project();
        assert_eq!(project, editor.buffer(b).unwrap().project());
        assert_eq!(editor.buffer_project(a).unwrap().found().worktree_of, None);
    }

    #[test]
    fn an_answer_for_a_buffer_that_moved_since_is_dropped() {
        let mut editor = Editor::with_buffer(visited("/w/oso/a.rs"));
        let a = editor.tiled().0.buffer();
        let request = asked(&mut editor, a);
        editor
            .buffer_mut(a)
            .unwrap()
            .set_path(PathBuf::from("/elsewhere/a.rs"));
        editor.project_found(&request, Some(found("/w/oso", 7)));
        assert_eq!(editor.buffer(a).unwrap().project(), None);
    }

    #[test]
    fn no_project_clears_it() {
        let mut editor = Editor::with_buffer(visited("/w/oso/a.rs"));
        let a = editor.tiled().0.buffer();
        editor.set_buffer_project(a, Some(found("/w/oso", 7)));
        editor.set_buffer_project(a, None);
        assert_eq!(editor.buffer(a).unwrap().project(), None);
        assert!(editor.buffer_project(a).is_none());
    }
}
