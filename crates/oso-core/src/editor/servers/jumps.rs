//! Jumps a language server's answers lead to, and back (ADR 0138, Go to definition and back,
//! step 8): where an ask about the selected buffer is, a jump to a place in an open buffer or
//! in a file to visit, and `M-,` and `C-M-,` through the selected window's history of jumps.
//!
//! Must not depend on: the host's servers, threads or I/O; the host asks the server and hands
//! over where its answer leads.
//!
//! A jump keeps the place it leaves in the window's history and on the mark ring there, as
//! avy's jumps do, and ends with one cursor at the target, with the place before on the
//! target's mark ring, as a visit at a place does (ADR 0114).

use std::path::PathBuf;

use crate::buffer::BufferId;
use crate::rope::Rope;
use crate::selection::OffsetEdit;
use crate::sum_tree::Bias;

use super::super::Editor;

/// A column on a line, as what gave it counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Column {
    /// In characters, as a person counts them.
    Chars(usize),
    /// In UTF-8 bytes.
    Utf8(usize),
    /// In UTF-16 code units, the protocol's default.
    Utf16(usize),
}

/// Where an ask about a buffer is: its primary cursor in the selected window, in the text of
/// `revision`, and whether the agent's command asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct At {
    pub buffer: BufferId,
    pub revision: u64,
    pub offset: usize,
    pub agent: bool,
}

impl Editor {
    /// Where an ask about the selected buffer is now.
    pub fn ask_at(&self) -> At {
        let (window, buffer) = self.current();
        At {
            buffer: window.buffer(),
            revision: buffer.revision(),
            offset: window.selections.primary().cursor,
            agent: self.runs_for_agent(),
        }
    }

    /// Jumps the selected window to `offset` in `buffer`, an open buffer, clipped to its text
    /// and to a display cluster's start. Does nothing for a buffer that does not exist.
    pub fn jump_to_place(&mut self, buffer: BufferId, offset: usize) {
        if self.buffer(buffer).is_none() {
            return;
        }
        self.keep_jump_start();
        self.go_to(buffer, offset);
    }

    /// Jumps the selected window to `line`, from 1, and `column` in `path`, visited as
    /// [`Editor::request_visit_at`] visits it, reading at most `limit` bytes.
    pub fn jump_visit(&mut self, path: PathBuf, limit: u64, line: usize, column: Column) {
        self.keep_jump_start();
        self.request_visit_at_column(path, limit, line, column);
    }

    /// `M-,`: back to the nearest place in the selected window's history. `Err` says why not.
    pub fn jump_back(&mut self) -> Result<(), &'static str> {
        let here = self.here();
        let place = self.windows[self.selected]
            .jumps_mut()
            .back(here)
            .ok_or("No earlier place")?;
        self.go_to(place.0, place.1);
        Ok(())
    }

    /// `C-M-,`: forward again to the place the last `M-,` left. `Err` says why not.
    pub fn jump_forward(&mut self) -> Result<(), &'static str> {
        let here = self.here();
        let place = self.windows[self.selected]
            .jumps_mut()
            .forward(here)
            .ok_or("No later place")?;
        self.go_to(place.0, place.1);
        Ok(())
    }

    /// Moves the places in `buffer` of every window's history through an edit's `steps`.
    pub(in crate::editor) fn jumps_follow(&mut self, buffer: BufferId, steps: &[Vec<OffsetEdit>]) {
        for window in &mut self.windows {
            window.jumps_mut().follow(buffer, steps);
        }
    }

    fn here(&self) -> (BufferId, usize) {
        let window = &self.windows[self.selected];
        (window.buffer(), window.selections.primary().cursor)
    }

    /// Keeps the selected window's place in its history and on its mark ring, as a jump
    /// leaves it: for a jump the host makes in its own way, as the agent channel's open.
    pub fn keep_jump_start(&mut self) {
        let here = self.here();
        let window = &mut self.windows[self.selected];
        window.jumps_mut().push(here);
        window.selections.push_marks();
    }

    /// Shows `buffer` in the selected window with one cursor at `offset`, the place before on
    /// its mark ring.
    fn go_to(&mut self, buffer: BufferId, offset: usize) {
        let Some(shown) = self.buffer(buffer) else {
            return;
        };
        let display = shown.display();
        let target =
            crate::motion::cluster_start(&display, shown.text().clip_offset(offset, Bias::Left));
        let index = self.selected;
        if self.windows[index].buffer() != buffer {
            self.show(buffer);
            if self.windows[index].buffer() != buffer {
                return;
            }
            self.windows[index].selections.push_marks();
        }
        let selections = &mut self.windows[index].selections;
        selections.keep_primary();
        selections.move_primary(target);
        self.follow_window(index);
    }
}

/// The offset of `column` on the line `row`, from 0, each as far as the text goes. O(log n),
/// as the line may be 16 MiB long.
pub(in crate::editor) fn column_offset(text: &Rope, row: usize, column: Column) -> usize {
    let row = row.min(text.line_count().saturating_sub(1));
    let (start, end) = (text.line_start(row), text.line_end(row));
    let offset = match column {
        Column::Chars(chars) => text.char_to_offset(text.chars_before(start).saturating_add(chars)),
        Column::Utf8(bytes) => text.clip_offset(start.saturating_add(bytes).min(end), Bias::Left),
        Column::Utf16(units) => {
            text.utf16_to_offset(text.offset_to_utf16(start).saturating_add(units))
        }
    };
    offset.min(end)
}

#[cfg(test)]
mod tests;
