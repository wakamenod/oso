//! `TAB` at every cursor of the selected window: spaces up to the next tab stop (ADR 0056,
//! ADR 0101).
//!
//! Must not depend on: `command` or `keymap`; the command calls this.

use super::Editor;
use crate::buffer::{EditError, Replacement};

impl Editor {
    /// Inserts spaces at every cursor of the selected window up to the next tab stop, as
    /// one edit. A cursor on the line of the one before takes its column from where the
    /// one before now ends, which the display map tells without reading what lies between,
    /// in O(log n) (ADR 0101). A read-only buffer refuses it and nothing changes.
    pub fn tab_to_tab_stop(&mut self) -> Result<(), EditError> {
        let (window, buffer) = self.current();
        let display = buffer.display();
        let tab_width = buffer.tab_width();
        let spaces = " ".repeat(tab_width.get() as usize);
        let mut edits = Vec::with_capacity(window.selections.len());
        // The cursor before this one, its line, and the column it ends at.
        let mut earlier: Option<(usize, usize, usize)> = None;
        for selection in window.selections.iter() {
            let line = buffer.text().offset_to_point(selection.cursor).row;
            // What lies between keeps its cells, but a tab reaches the next stop from where it
            // now starts, which TabMap tells without reading what lies between, counting from
            // the line's start (ADR 0101).
            let column = match earlier {
                Some((from, row, column)) if row == line => {
                    display.column_after(from, column, selection.cursor)
                }
                _ => display.offset_to_display_point(selection.cursor).column,
            };
            let stop = tab_width.next_stop(column);
            edits.push(Replacement {
                range: selection.cursor..selection.cursor,
                text: &spaces[..stop - column],
            });
            earlier = Some((selection.cursor, line, stop));
        }
        let id = window.buffer();
        self.apply_edits(id, &edits)
    }
}

#[cfg(test)]
mod tests {
    use super::super::edit_tests::{cursors, editor_with, set_at, text};
    use crate::buffer::{Buffer, EditError, ReadOnly};
    use crate::editor::Editor;

    fn columns(editor: &Editor) -> Vec<usize> {
        let (window, buffer) = editor.current();
        let display = buffer.display();
        let selections = window.selections.iter();
        selections
            .map(|selection| display.offset_to_display_point(selection.cursor).column)
            .collect()
    }

    #[test]
    fn each_cursor_reaches_its_stop_counting_what_earlier_ones_insert() {
        let cases: [(&str, &[usize], &[usize]); 6] = [
            ("abcdef", &[1, 3], &[8, 16]),
            ("abcdef", &[0, 1, 2], &[8, 16, 24]),
            // Wide characters take their cells, whatever their bytes.
            ("日本語の文章", &[0, 6, 18], &[8, 16, 32]),
            // A tab between them still reaches a stop, from where it now starts.
            ("a\tb", &[1, 3], &[8, 24]),
            // A cursor on another line starts from its own column.
            ("ab\ncd", &[1, 4], &[8, 8]),
            ("", &[0], &[8]),
        ];
        for (start, places, expected) in cases {
            let (mut editor, _) = editor_with(start);
            editor.windows[0].selections = set_at(places);
            editor.tab_to_tab_stop().unwrap();
            assert_eq!(columns(&editor), expected, "{start:?} at {places:?}");
        }

        let (mut editor, id) = editor_with("abcdef");
        editor.windows[0].selections = set_at(&[1, 3]);
        editor.tab_to_tab_stop().unwrap();
        let spaces = |n| " ".repeat(n);
        assert_eq!(
            text(&editor, id),
            format!("a{}bc{}def", spaces(7), spaces(6))
        );
        assert_eq!(cursors(&editor.windows[0]), [8, 16]);
    }

    #[test]
    fn a_read_only_buffer_refuses_it_and_nothing_changes() {
        let mut editor = Editor::with_buffer(Buffer::from_bytes("bad", b"a\xff"));
        let id = editor.window().buffer();
        let revision = editor.buffer(id).unwrap().revision();
        assert_eq!(
            editor.tab_to_tab_stop(),
            Err(EditError::ReadOnly(ReadOnly::InvalidUtf8))
        );
        assert_eq!(editor.buffer(id).unwrap().revision(), revision);
    }
}
