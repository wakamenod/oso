//! The directory each buffer stands in, as Emacs's `default-directory` is (ADR 0141): where a
//! prompt that reads a path starts, and where a terminal or a session started from the buffer
//! runs when no project says.
//!
//! Must not depend on: `command` or `keymap`, or the host's files, threads or processes; the
//! host gives a terminal's, a session's and the side pane's directories.
//!
//! - A buffer that visits a file stands in the file's directory, a listing in the directory it
//!   lists, and a git status or a review in its project's root, each read anew, so that a write
//!   under a new name or a move is followed.
//! - Any other buffer stands where it was made: in the directory of the buffer shown then, in
//!   the window commands act on, as Emacs's new buffers take the current one's. The host gives
//!   a terminal the one it started in, a session its own, and the side pane the row at point's.
//! - What a terminal's program says of its directory (OSC 7) moves where a prompt for a path
//!   starts in that terminal and nothing else: a buffer made from it takes the one it started
//!   in, so that a program's output never reaches where a process starts.
//! - A buffer with none of these, such as `*scratch*`, stands in the directory oso started in.

use std::path::{Path, PathBuf};

use super::Editor;
use crate::buffer::BufferId;

impl Editor {
    /// The directory the buffer `id` stands in (ADR 0141): its file's, the one a listing lists,
    /// a git status's or a review's project root, else the one it was made in or the host gave
    /// it, else the directory oso started in.
    pub fn default_directory(&self, id: BufferId) -> PathBuf {
        let reported = self
            .buffer(id)
            .and_then(crate::buffer::Buffer::reported_directory);
        (reported.map(Path::to_path_buf))
            .or_else(|| self.own_directory(id))
            .unwrap_or_else(|| self.start_directory.clone())
    }

    /// Gives the buffer `id` the directory it stands in where no file, listing or project says
    /// one: the one a terminal started in, a session's, the place of the side pane's row at
    /// point (ADR 0141). Nothing happens to a buffer not there.
    pub fn set_buffer_directory(&mut self, id: BufferId, directory: PathBuf) {
        if let Some(buffer) = self.buffer_mut(id) {
            buffer.set_directory(Some(directory));
        }
    }

    /// The program of the terminal buffer `id` says it is in `directory` (OSC 7): a prompt for
    /// a path starts there, and [`Editor::default_directory`] says it, but a buffer made from it
    /// stands where the terminal started (ADR 0141). Nothing happens to a buffer that is not a
    /// terminal's.
    pub fn set_reported_directory(&mut self, id: BufferId, directory: PathBuf) {
        if let Some(buffer) = self.buffer_mut(id).filter(|buffer| buffer.is_terminal()) {
            buffer.set_reported_directory(directory);
        }
    }

    /// The directory the buffer `id` stands in, unless it falls back to the one oso started in,
    /// leaving out what a terminal's program said.
    fn own_directory(&self, id: BufferId) -> Option<PathBuf> {
        let buffer = self.buffer(id)?;
        let parent = buffer
            .path()
            .and_then(Path::parent)
            .filter(|parent| !parent.as_os_str().is_empty());
        if let Some(parent) = parent {
            return Some(parent.to_path_buf());
        }
        if let Some(listing) = self.listing(id) {
            return Some(listing.directory.clone());
        }
        let reviewed = self.review_of(id).and_then(|review| {
            self.reviews()
                .find_map(|(other, project)| (other == review).then_some(project))
        });
        let project = self.git_status_of(id).or(reviewed);
        if let Some(project) = project.and_then(|project| self.project(project)) {
            return Some(project.root().to_path_buf());
        }
        // A hosted session's, which its project is found from (ADR 0132).
        (buffer.session_directory().or_else(|| buffer.directory())).map(Path::to_path_buf)
    }

    /// Gives `buffer`, about to be added, the directory it stands in unless it has one: that of
    /// the buffer shown now, as Emacs's new buffers take the current one's.
    pub(super) fn stand_where_made(&self, buffer: &mut crate::buffer::Buffer) {
        if buffer.directory().is_none() {
            buffer.set_directory(self.inherited_directory());
        }
    }

    /// The directory a buffer made now takes: that of the buffer shown in the window commands
    /// act on, beneath any prompt. `None` before the first window, or where it stands in the
    /// directory oso started in, which it then follows too.
    fn inherited_directory(&self) -> Option<PathBuf> {
        let here = match self.prompts.first() {
            Some(prompt) => prompt.beneath,
            None => self.windows.get(self.selected)?.id(),
        };
        self.own_directory(self.window_by_id(here)?.buffer())
    }

    /// The buffers that stand in `from`, or in a directory under it, stand in the same place
    /// under `to`, as it was renamed or moved there (ADR 0134).
    pub(super) fn directories_moved(&mut self, from: &Path, to: &Path) {
        for (_, buffer) in &mut self.buffers {
            let Some(rest) = buffer
                .directory()
                .and_then(|dir| dir.strip_prefix(from).ok())
            else {
                continue;
            };
            let moved = if rest.as_os_str().is_empty() {
                to.to_path_buf()
            } else {
                to.join(rest)
            };
            buffer.set_directory(Some(moved));
        }
    }
}

#[cfg(test)]
mod tests;
