//! What commands ask of the host about agent sessions (ADR 0121), and the text of the internal
//! buffers the host fills for them, such as `*Session explain*`.
//!
//! Must not depend on: the host's threads or I/O. A command asks; the host takes the request
//! after the command, does the work on its sessions worker, and fills the buffer.

use std::path::PathBuf;

use super::Editor;
use crate::buffer::{BufferId, Replacement};
use crate::selection::SelectionSet;

/// What a command asks of the host about agent sessions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionRequest {
    /// Explain the saved screen in this file with the rules in force (`session-explain-file`).
    ExplainFile(PathBuf),
}

impl Editor {
    /// A command asks the host for `request`.
    pub fn request_session(&mut self, request: SessionRequest) {
        self.session_requests.push(request);
    }

    /// What commands asked about sessions since the host last looked, oldest first.
    pub fn take_session_requests(&mut self) -> Vec<SessionRequest> {
        std::mem::take(&mut self.session_requests)
    }

    /// Replaces the whole text of the internal buffer `id` with `text`, with no undo record,
    /// and puts the cursors of the windows showing it at its start. Nothing happens to a buffer
    /// that is not internal. It costs one pass over the old text and the new: a report the host
    /// writes, such as `*Session explain*`'s, of some 50 KiB at most.
    pub fn set_internal_text(&mut self, id: BufferId, text: &str) {
        let Some(buffer) = self.buffer(id).filter(|buffer| buffer.is_internal()) else {
            return;
        };
        let edits = [Replacement {
            range: 0..buffer.text().len(),
            text,
        }];
        let Some(staged) = buffer.stage_internal(&edits) else {
            return;
        };
        let views = self.moved_views(id, &staged.display(), staged.steps(), staged.rewrap());
        // Nothing past this point can panic part way.
        if let Some(buffer) = self.buffer_mut(id) {
            buffer.commit_internal(staged);
        }
        let start = SelectionSet::new(0);
        for window in &mut self.windows {
            if window.buffer() == id {
                window.selections = start.clone();
            }
        }
        self.commit_views(id, views);
        if self.window().buffer() == id {
            self.follow_cursor();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::SessionRequest;
    use crate::buffer::{Buffer, Mode};
    use crate::editor::Editor;

    #[test]
    fn requests_are_taken_once_in_order() {
        let mut editor = Editor::default();
        assert!(editor.take_session_requests().is_empty());
        editor.request_session(SessionRequest::ExplainFile(PathBuf::from("a")));
        editor.request_session(SessionRequest::ExplainFile(PathBuf::from("b")));
        assert_eq!(
            editor.take_session_requests(),
            [
                SessionRequest::ExplainFile(PathBuf::from("a")),
                SessionRequest::ExplainFile(PathBuf::from("b")),
            ]
        );
        assert!(editor.take_session_requests().is_empty());
    }

    #[test]
    fn an_internal_buffers_text_is_replaced_with_the_cursor_at_its_start() {
        let mut editor = Editor::default();
        let id = editor.add_internal_buffer("*Report*", Mode::Fundamental);
        editor.set_internal_text(id, "first\nreport\n");
        editor.show(id);
        for window in &mut editor.windows {
            window.selections = crate::selection::SelectionSet::new(8);
        }
        editor.set_internal_text(id, "second");
        let buffer = editor.buffer(id).unwrap();
        assert_eq!(buffer.text().to_string(), "second");
        assert!(!buffer.is_modified());
        assert_eq!(editor.window().selections.primary().cursor, 0);
    }

    #[test]
    fn a_withheld_buffer_counts_as_secret_from_then_on() {
        let mut editor = Editor::default();
        let id = editor.add_internal_buffer("*Report*", Mode::Fundamental);
        assert!(!editor.buffer(id).unwrap().is_secret());
        editor.buffer_mut(id).unwrap().withhold();
        editor.set_internal_text(id, "screen text");
        assert!(editor.buffer(id).unwrap().is_secret());
    }

    #[test]
    fn a_buffer_that_is_not_internal_is_left_alone() {
        let mut editor = Editor::default();
        let id = editor.add_buffer(Buffer::from_text("notes", "mine"));
        editor.set_internal_text(id, "replaced");
        assert_eq!(editor.buffer(id).unwrap().text().to_string(), "mine");
    }
}
