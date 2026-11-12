//! Visiting files: the visits commands ask the host for, the file a load stopped as too large,
//! which the next visit of it asks about (ADR 0087), and a visit at a place (ADR 0114): once the
//! file's buffer shows with that line, the selected window goes to it, with the place before on
//! the mark ring.
//!
//! Must not depend on: `command` or `keymap`, or the host's threads or I/O; the host loads the
//! file and says when text came.

use std::path::{Path, PathBuf};

use super::Editor;
use crate::buffer::{Buffer, BufferId};
use crate::rope::Rope;

/// A file to go to a place in once a visit shows it, with the line and column.
pub(super) type VisitPlace = (PathBuf, usize, usize);

impl Editor {
    /// Asks the host to visit `path` in the selected window, reading at most `limit` bytes, as a
    /// command cannot start a load itself. A buffer that visits that path already is shown at
    /// once instead, unless the agent channel is still opening it.
    pub fn request_visit(&mut self, path: PathBuf, limit: u64) {
        self.visit_place = None;
        match self.open_buffer(|_, buffer| buffer.path() == Some(path.as_path())) {
            Some(id) => self.show(id),
            None => self.visits.push((path, limit)),
        }
    }

    /// The first open buffer that `wanted` picks, by its id and itself, of those that show what
    /// they hold: none that the agent channel is still opening, as the channel checks what it
    /// may show before it shows anything (ADR 0085). Once shown, it is like any other.
    pub(super) fn open_buffer(
        &self,
        mut wanted: impl FnMut(BufferId, &Buffer) -> bool,
    ) -> Option<BufferId> {
        self.buffers()
            .find(|(id, buffer)| !buffer.agent_pending() && wanted(*id, buffer))
            .map(|(id, _)| id)
    }

    /// The visits commands asked for, for the host to start.
    pub fn take_visits(&mut self) -> Vec<(PathBuf, u64)> {
        std::mem::take(&mut self.visits)
    }

    /// Remembers that the file at `path` held `bytes` when a load stopped before reading it, so
    /// that the next `C-x C-f` of it asks (ADR 0087).
    pub fn remember_large(&mut self, path: PathBuf, bytes: u64) {
        self.large = Some((path, bytes));
    }

    /// The size the file at `path` had when a load stopped before it, if that is the one
    /// remembered.
    pub fn large_file(&self, path: &Path) -> Option<u64> {
        self.large
            .as_ref()
            .filter(|(large, _)| large == path)
            .map(|(_, bytes)| *bytes)
    }

    /// Takes the large file remembered, as a question about it is answered.
    pub fn take_large(&mut self) -> Option<(PathBuf, u64)> {
        self.large.take()
    }

    /// Asks the host to visit `path` in the selected window, as [`Editor::request_visit`]
    /// does, and once it shows, to go to `line`, from 1, and `column`, in characters from 0,
    /// each as far as the text goes, with the place before on the mark ring.
    pub fn request_visit_at(&mut self, path: PathBuf, limit: u64, line: usize, column: usize) {
        self.request_visit(path.clone(), limit);
        self.visit_place = Some((path, line, column));
        let (_, buffer) = self.current();
        let shown = buffer.path() == Some(self.visit_place_path()) && buffer.loading().is_none();
        if shown {
            let id = self.windows[self.selected].buffer();
            self.go_to_visit_place(id);
        }
    }

    fn visit_place_path(&self) -> &Path {
        self.visit_place
            .as_ref()
            .map_or(Path::new(""), |(path, _, _)| path)
    }

    /// Goes to the place a visit asked for, in the window showing `id`, if `id` visits its file
    /// and has loaded, or has loaded as far as the line.
    pub(super) fn go_to_visit_place(&mut self, id: BufferId) {
        let Some((path, line, column)) = &self.visit_place else {
            return;
        };
        let Some(buffer) = self.buffer(id) else {
            return;
        };
        if buffer.path() != Some(path.as_path()) {
            return;
        }
        let text = buffer.text();
        let row = line.saturating_sub(1);
        // The whole line has come, or the file has.
        if buffer.loading().is_some() && row + 1 >= text.line_count() {
            return;
        }
        let offset = place_offset(text, row, *column);
        self.visit_place = None;
        let index = self.selected;
        if index >= self.tiled_count() || self.windows[index].buffer() != id {
            return;
        }
        let selections = &mut self.windows[index].selections;
        selections.keep_primary();
        selections.push_marks();
        selections.move_primary(offset);
        self.follow_window(index);
    }
}

/// The offset of `column`, in characters from 0, on the line `row`, from 0, each as far as the
/// text goes. O(log n), as the line may be 16 MiB long.
/// The device and inode of the file `buffer` last loaded or saved, where the system gave them.
pub(super) fn file_identity(buffer: &Buffer) -> Option<(u64, u64)> {
    buffer.baseline()?.file?.identity
}

pub(super) fn place_offset(text: &Rope, row: usize, column: usize) -> usize {
    let row = row.min(text.line_count().saturating_sub(1));
    let (start, end) = (text.line_start(row), text.line_end(row));
    let chars = text.chars_before(start).saturating_add(column);
    text.char_to_offset(chars).min(end)
}

#[cfg(test)]
mod tests;
