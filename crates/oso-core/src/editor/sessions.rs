//! What commands ask of the host about agent sessions (ADR 0121).
//!
//! Must not depend on: the host's threads or I/O. A command asks; the host takes the request
//! after the command, does the work on its sessions worker, and fills an internal buffer with
//! [`Editor::set_internal_text`].

use std::path::PathBuf;

use super::Editor;
use crate::buffer::BufferId;

/// What a command asks of the host about agent sessions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionRequest {
    /// Explain the saved screen in this file with the rules in force (`session-explain-file`).
    ExplainFile(PathBuf),
    /// Start a session (`claude`): in `directory`, else the selected window's project root,
    /// else the buffer's directory, named `name`, else after its directory.
    Start {
        name: Option<String>,
        directory: Option<PathBuf>,
    },
    /// Resume the stopped session whose terminal is this buffer (`session-resume`).
    Resume(BufferId),
    /// Copy the conversation of the session whose terminal is this buffer under a new id, in a
    /// new buffer (`session-fork`).
    Fork(BufferId),
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
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::SessionRequest;
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
}
