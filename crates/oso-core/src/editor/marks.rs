//! `C-.`: the primary cursor's mark and its ring as a list that completes, with the buffer
//! around the selected mark in the preview, and going there (ADR 0071, ADR 0088).
//!
//! Must not depend on: `command` or `keymap`, or the host's window.

use std::sync::Arc;

use super::Editor;
use super::completion::CompletionStyle;
use super::messages::row_text;
use crate::buffer::BufferId;
use crate::complete::Candidate;
use crate::minibuffer::{PromptError, PromptId};

/// The most bytes of a mark's line a row shows.
const LINE_BYTES: usize = 200;

/// The marks a `C-.` prompt lists: the buffer they are in, and each row's text with its offset.
#[derive(Debug)]
pub(super) struct MarkList {
    prompt: PromptId,
    buffer: BufferId,
    marks: Vec<(String, usize)>,
}

impl Editor {
    /// Opens a prompt that says `label` and completes over the selected window's marks, the
    /// primary cursor's mark first and then its ring, newest first. A row is the mark's line
    /// and column, then that line's text, escaped as `*Messages*` escapes a row. It keeps no
    /// history.
    ///
    /// # Errors
    ///
    /// [`PromptError::Invalid`] with no mark, and [`PromptError::TooDeep`] with as many
    /// prompts open as may be. Nothing changes.
    pub fn open_mark_list(&mut self, label: &str) -> Result<PromptId, PromptError> {
        let (window, buffer) = self.current();
        let selections = &window.selections;
        let text = buffer.text();
        let offsets = selections
            .primary()
            .mark
            .into_iter()
            .chain(selections.mark_ring().iter());
        let mut marks: Vec<(String, usize)> = Vec::new();
        for offset in offsets {
            let offset = offset.min(text.len());
            let point = text.offset_to_point(offset);
            let start = text.line_start(point.row);
            let end = text.line_end(point.row).min(start + LINE_BYTES);
            let line = text.slice(start..end).to_string();
            let row = format!("{}:{}: {}", point.row + 1, point.column, row_text(&line));
            if !marks.iter().any(|(other, _)| *other == row) {
                marks.push((row, offset));
            }
        }
        if marks.is_empty() {
            return Err(PromptError::Invalid);
        }
        let id = window.buffer();
        let items: Arc<[Candidate]> = marks
            .iter()
            .map(|(row, _)| Candidate::new(row.as_str(), "", ""))
            .collect();
        let style = CompletionStyle {
            given_order: true,
            takes_input: false,
            preview: true,
        };
        // No history: the rows are this buffer's text.
        let prompt = self.open_completion_as(label, "", items, style)?;
        self.mark_list = Some(MarkList {
            prompt,
            buffer: id,
            marks,
        });
        Ok(prompt)
    }

    /// The buffer and the offset of the mark the innermost prompt's selected row names, while
    /// that prompt is a `C-.` list of the buffer the tiled window shows.
    pub(super) fn selected_mark(&self) -> Option<(BufferId, usize)> {
        let list = self.mark_list.as_ref()?;
        let prompt = self.prompt().filter(|prompt| prompt.id() == list.prompt)?;
        if self.tiled().0.buffer() != list.buffer {
            return None;
        }
        let row = prompt.candidates()?.selected_candidate()?.text();
        let offset = list.marks.iter().find(|(text, _)| text == row)?.1;
        Some((list.buffer, offset))
    }

    /// Goes to the mark that the row `answer` of a `C-.` list names by its line and column,
    /// clipped to the text as it is now, with one cursor and the region hidden, and sets the mark
    /// where the cursor was, which puts the old mark in the ring, as `C-u C-SPC` keeps where it
    /// came from.
    ///
    /// # Errors
    ///
    /// [`PromptError::Invalid`] if `answer` names no line and column.
    pub fn go_to_listed_mark(&mut self, answer: &str) -> Result<(), PromptError> {
        let mut parts = answer.splitn(3, ':');
        let line: usize = parts
            .next()
            .and_then(|part| part.parse().ok())
            .ok_or(PromptError::Invalid)?;
        let column: usize = parts
            .next()
            .and_then(|part| part.parse().ok())
            .ok_or(PromptError::Invalid)?;
        let (window, buffer) = self.current_mut();
        let text = buffer.text();
        let row = line.checked_sub(1).ok_or(PromptError::Invalid)?;
        if row > text.offset_to_point(text.len()).row {
            return Err(PromptError::Invalid);
        }
        let start = text.line_start(row);
        let offset = (start + column).min(text.line_end(row));
        let display = buffer.display();
        let offset = crate::motion::cluster_start(&display, offset);
        let selections = &mut window.selections;
        let from = selections.primary().cursor;
        selections.keep_primary();
        selections.push_primary_mark(from);
        selections.move_primary(offset);
        selections.set_mark_active(crate::selection::MarkActive::Inactive);
        self.follow_cursor();
        Ok(())
    }

    /// Forgets a `C-.` list whose prompt `closed`.
    pub(super) fn forget_marks(&mut self, closed: PromptId) {
        if self
            .mark_list
            .as_ref()
            .is_some_and(|list| list.prompt == closed)
        {
            self.mark_list = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;

    #[test]
    fn the_marks_are_listed_newest_first_and_going_to_one_keeps_where_it_came_from() {
        let mut editor = Editor::with_buffer(Buffer::from_text("a", "one\ntwo\nthree\n"));
        let selections = &mut editor.current_mut().0.selections;
        selections.push_primary_mark(4);
        selections.push_primary_mark(8);
        selections.move_primary(2);
        editor.open_mark_list("Go to mark:").unwrap();
        let rows: Vec<String> = {
            let candidates = editor.prompt().unwrap().candidates().unwrap();
            candidates
                .shown(18)
                .map(|(_, candidate)| candidate.text().to_owned())
                .collect()
        };
        assert_eq!(rows, ["3:0: three", "2:0: two"]);
        assert_eq!(editor.selected_mark().map(|(_, offset)| offset), Some(8));
        editor.quit();
        editor.go_to_listed_mark("2:0: two").unwrap();
        let selections = &editor.window().selections;
        assert_eq!(selections.primary().cursor, 4);
        assert_eq!(selections.primary().mark, Some(2), "where it came from");
    }

    #[test]
    fn no_mark_lists_nothing() {
        let mut editor = Editor::with_buffer(Buffer::from_text("a", "one\n"));
        assert_eq!(
            editor.open_mark_list("Go to mark:"),
            Err(PromptError::Invalid)
        );
        assert!(editor.prompt().is_none());
    }
}
