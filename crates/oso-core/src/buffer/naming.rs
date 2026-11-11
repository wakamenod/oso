//! A buffer's name and path as the editor changes them: the names that tell files with one
//! name apart, a write under a new name, and the project the file is in (ADR 0087, ADR 0088,
//! ADR 0114).
//!
//! Must not depend on: `editor`.

use std::path::PathBuf;

use super::Buffer;
use crate::project::ProjectId;

impl Buffer {
    /// Renames the buffer, as the editor tells buffers of files with one name apart.
    pub(crate) fn set_name(&mut self, name: String) {
        if self.name != name {
            self.name = name;
            self.naming = self.naming.wrapping_add(1);
        }
    }

    /// Makes the buffer visit `path`, as a write under a new name does. What was known of
    /// the old file goes: its baseline, its own spelling, its project and what a save found
    /// there.
    pub(crate) fn set_path(&mut self, path: PathBuf) {
        self.path = Some(path);
        self.baseline = None;
        self.own_spelling = None;
        self.project = None;
        self.changed_on_disk = false;
        self.save_over = None;
        self.naming = self.naming.wrapping_add(1);
    }

    /// Changes whenever the name or the path does.
    pub fn naming(&self) -> u32 {
        self.naming
    }

    /// The project its file is in, once the host has found it. A buffer with no file, or a
    /// file in no project, has none.
    pub fn project(&self) -> Option<ProjectId> {
        self.project
    }

    pub(crate) fn set_project(&mut self, project: Option<ProjectId>) {
        self.project = project;
    }
}
