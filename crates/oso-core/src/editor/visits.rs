//! Visiting a file at a place (ADR 0114): a visit a command asks for, and once the file's buffer
//! shows with that line, the selected window going to it, with the place before on the mark
//! ring.
//!
//! Must not depend on: `command` or `keymap`, or the host's threads or I/O; the host loads the
//! file and says when text came.

use std::path::{Path, PathBuf};

use super::Editor;
use crate::buffer::BufferId;

/// A file to go to a place in once a visit shows it, with the line and column.
pub(super) type VisitPlace = (PathBuf, usize, usize);

impl Editor {
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
        let row = row.min(text.line_count() - 1);
        let (start, end) = (text.line_start(row), text.line_end(row));
        let line_text = text.slice(start..end).to_string();
        let offset = start
            + line_text
                .char_indices()
                .nth(*column)
                .map_or(line_text.len(), |(at, _)| at);
        self.visit_place = None;
        let Some(index) = (0..self.tiled_count())
            .find(|&index| index == self.selected && self.windows[index].buffer() == id)
        else {
            return;
        };
        let selections = &mut self.windows[index].selections;
        selections.keep_primary();
        selections.push_marks();
        selections.move_primary(offset);
        self.follow_window(index);
    }
}

#[cfg(test)]
mod tests;
