//! A buffer's name and path as the editor changes them: the names that tell files with one
//! name apart, a write under a new name, the project the file is in, and the directory it
//! stands in (ADR 0087, ADR 0088, ADR 0114, ADR 0141).
//!
//! Must not depend on: `editor`.

use std::path::{Path, PathBuf};

use super::{Buffer, Mode};
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
    /// there. The new name gives the mode, as a visit does (ADR 0111).
    pub(crate) fn set_path(&mut self, path: PathBuf) {
        // A name never gives the copies' modes, and an internal buffer is never written.
        self.set_mode(Mode::for_path(&path));
        self.path = Some(path);
        self.baseline = None;
        self.own_spelling = None;
        self.project = None;
        self.changed_on_disk = false;
        self.save_over = None;
        self.naming = self.naming.wrapping_add(1);
    }

    /// Makes the buffer visit its file at `path`, where it was moved (ADR 0134): its text, edits
    /// and baseline stay, the baseline at the new place, and it is judged by its new name,
    /// never less secret than before. Its project is looked for again.
    pub(crate) fn moved_to(&mut self, path: PathBuf) {
        let stricter = crate::secret::judge_name(&path).is_withheld();
        if let Some(baseline) = self.baseline.as_mut() {
            baseline.location.clone_from(&path);
        }
        self.path = Some(path);
        self.own_spelling = None;
        self.project = None;
        self.naming = self.naming.wrapping_add(1);
        if stricter {
            self.judge_moved_secret();
        }
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

    /// The directory it stands in when no file, listing or project says one, if it has one of
    /// its own (ADR 0141).
    pub fn directory(&self) -> Option<&Path> {
        self.directory.as_deref()
    }

    pub(crate) fn set_directory(&mut self, directory: Option<PathBuf>) {
        self.directory = directory;
    }
}
