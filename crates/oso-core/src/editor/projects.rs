//! The projects buffers are in (ADR 0114): a table the host fills as it finds each file's
//! project, which buffers point into.
//!
//! Must not depend on: the host's threads or I/O. The host finds a project on the thread that
//! loads the file, and hands over what it found.

use super::Editor;
use crate::buffer::BufferId;
use crate::project::{Found, Project, ProjectId};

impl Editor {
    /// The host found that `id`'s file is in the project `found`, or in none. A project
    /// already in the table, by its root and the directory's identity, is shared; any other
    /// is added.
    pub fn set_buffer_project(&mut self, id: BufferId, found: Option<Found>) {
        if self.buffer(id).is_none() {
            return;
        }
        let project = found.map(|found| self.intern_project(found));
        if let Some(buffer) = self.buffer_mut(id) {
            buffer.set_project(project);
        }
    }

    /// The project `id` names.
    pub fn project(&self, id: ProjectId) -> Option<&Project> {
        self.projects.get(usize::try_from(id.0).ok()?)
    }

    /// The project of the buffer `id`, if its file is in one.
    pub fn buffer_project(&self, id: BufferId) -> Option<&Project> {
        self.project(self.buffer(id)?.project()?)
    }

    fn intern_project(&mut self, found: Found) -> ProjectId {
        if let Some(place) = self
            .projects
            .iter()
            .position(|known| *known.found() == found)
        {
            return ProjectId(u32::try_from(place).unwrap_or(u32::MAX));
        }
        self.projects.push(Project::new(found));
        ProjectId(u32::try_from(self.projects.len() - 1).unwrap_or(u32::MAX))
    }
}

#[cfg(test)]
mod tests {
    use crate::buffer::Buffer;
    use crate::editor::Editor;
    use crate::project::{Found, RootIdentity};
    use std::path::PathBuf;

    fn found(path: &str, inode: u64) -> Found {
        Found {
            root: RootIdentity {
                path: PathBuf::from(path),
                identity: Some((1, inode)),
            },
            trust_root: None,
        }
    }

    #[test]
    fn buffers_in_one_project_share_it_and_others_get_their_own() {
        let mut editor = Editor::with_buffer(Buffer::visiting(PathBuf::from("/w/oso/a.rs")));
        let a = editor.tiled().0.buffer();
        let b = editor.add_buffer(Buffer::visiting(PathBuf::from("/w/oso/b.rs")));
        let c = editor.add_buffer(Buffer::visiting(PathBuf::from("/w/other/c.rs")));
        editor.set_buffer_project(a, Some(found("/w/oso", 7)));
        editor.set_buffer_project(b, Some(found("/w/oso", 7)));
        editor.set_buffer_project(c, Some(found("/w/other", 8)));
        let project = |id| editor.buffer(id).unwrap().project().unwrap();
        assert_eq!(project(a), project(b));
        assert_ne!(project(a), project(c));
        assert_eq!(editor.buffer_project(a).unwrap().name(), "oso");
        assert_eq!(editor.buffer_project(c).unwrap().name(), "other");
    }

    #[test]
    fn a_root_made_again_at_the_same_path_is_another_project() {
        let mut editor = Editor::with_buffer(Buffer::visiting(PathBuf::from("/w/oso/a.rs")));
        let a = editor.tiled().0.buffer();
        let b = editor.add_buffer(Buffer::visiting(PathBuf::from("/w/oso/b.rs")));
        editor.set_buffer_project(a, Some(found("/w/oso", 7)));
        editor.set_buffer_project(b, Some(found("/w/oso", 9)));
        assert_ne!(
            editor.buffer(a).unwrap().project(),
            editor.buffer(b).unwrap().project()
        );
    }

    #[test]
    fn no_project_clears_it() {
        let mut editor = Editor::with_buffer(Buffer::visiting(PathBuf::from("/w/oso/a.rs")));
        let a = editor.tiled().0.buffer();
        editor.set_buffer_project(a, Some(found("/w/oso", 7)));
        editor.set_buffer_project(a, None);
        assert_eq!(editor.buffer(a).unwrap().project(), None);
        assert!(editor.buffer_project(a).is_none());
    }
}
