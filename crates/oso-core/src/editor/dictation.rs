//! What the editor knows of dictation (ADR 0175): what the person asked of it, for the host,
//! and whether one records or waits for its text, for the modeline.
//!
//! Must not depend on: processes, threads or the host's dictation; the host says what holds,
//! and takes what was asked.

use super::Editor;

/// What a command asks of the host about dictation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DictationRequest {
    /// Start a dictation, or stop the one recording (`dictation-toggle`). `prefix` says a
    /// prefix argument came with the key.
    Toggle { prefix: bool },
    /// Drop the dictation under way and what it would type (`dictation-cancel`).
    Cancel,
}

/// What the modeline says of dictation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DictationMark {
    /// Nothing: no dictation is under way.
    #[default]
    Off,
    /// One records.
    Recording,
    /// One stopped recording and waits for its text.
    Waiting,
}

/// The editor's part of dictation.
#[derive(Debug, Default)]
pub(super) struct Dictation {
    requests: Vec<DictationRequest>,
    pub(super) mark: DictationMark,
}

impl Editor {
    /// A command asks the host to start, stop or cancel a dictation.
    pub fn request_dictation(&mut self, request: DictationRequest) {
        self.dictation.requests.push(request);
    }

    /// What commands asked of dictation since the host last looked, oldest first.
    pub fn take_dictation_requests(&mut self) -> Vec<DictationRequest> {
        std::mem::take(&mut self.dictation.requests)
    }

    /// The host says whether a dictation records or waits. It asks for a frame itself.
    pub fn set_dictation_mark(&mut self, mark: DictationMark) {
        self.dictation.mark = mark;
    }

    /// What the modeline says of dictation.
    pub fn dictation_mark(&self) -> DictationMark {
        self.dictation.mark
    }
}

#[cfg(test)]
mod tests {
    use super::{DictationMark, DictationRequest};
    use crate::editor::Editor;

    #[test]
    fn requests_go_to_the_host_once_in_order() {
        let mut editor = Editor::default();
        editor.request_dictation(DictationRequest::Toggle { prefix: true });
        editor.request_dictation(DictationRequest::Cancel);
        assert_eq!(
            editor.take_dictation_requests(),
            [
                DictationRequest::Toggle { prefix: true },
                DictationRequest::Cancel
            ]
        );
        assert!(editor.take_dictation_requests().is_empty());
        assert_eq!(editor.dictation_mark(), DictationMark::Off);
    }
}
