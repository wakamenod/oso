//! A native session's first message (ADR 0165, step 1): a person's start with no brief opens
//! `*Brief*` for it, a multi-line buffer withheld from agents, and the session starts once it
//! is sent. One brief is written at a time, a worktree session's or a native one's.
//!
//! Must not depend on: key presses or how a frame is drawn; the commands ask, and this carries
//! out.

use oso_core::buffer::{Buffer, BufferId, Mode};

use crate::app::App;
use crate::app::sessions::hosting::Begin;
use crate::app::sessions::withheld_draft;
use crate::sessions::native::queue::MAX_MESSAGE;

/// The buffer a native session's first message is written in.
const FIRST_MESSAGE: &str = "*Brief*";

impl App {
    /// Opens `*Brief*` for the first message of the native session `begin` starts: `C-c C-c`
    /// starts it with the message, `C-c C-k` drops it. One is written at a time, as a worktree
    /// session's brief.
    pub(super) fn write_first_message(&mut self, begin: Begin) {
        let open = (self.hosting.natives.briefs.keys())
            .chain(self.hosting.worktrees.brief_buffers())
            .next()
            .copied();
        if let Some(open) = open {
            self.editor.show(open);
            return self
                .editor
                .message("A brief is being written: send it with C-c C-c or drop it with C-c C-k");
        }
        let id = withheld_draft(&mut self.editor, FIRST_MESSAGE, Mode::Brief, "");
        self.editor.show(id);
        self.hosting.natives.briefs.insert(id, begin);
        self.editor.message(
            "Write the session's first message: C-c C-c starts the session, C-c C-k drops it",
        );
    }

    /// `brief-send` in a native session's `*Brief*`: the session starts with it as its first
    /// message. Whether `buffer` was one.
    pub(in crate::app) fn send_first_message(&mut self, buffer: BufferId) -> bool {
        if !self.hosting.natives.briefs.contains_key(&buffer) {
            return false;
        }
        let Some(text) = self.editor.buffer(buffer).map(Buffer::text) else {
            return true;
        };
        // Measured before it is copied.
        if text.len() > MAX_MESSAGE {
            self.editor.message("The message is over 64 KiB");
            return true;
        }
        let text = text.to_string();
        if text.trim().is_empty() {
            self.editor.message("The message is empty");
            return true;
        }
        let Some(begin) = self.hosting.natives.briefs.remove(&buffer) else {
            return true;
        };
        self.editor.kill_buffer(buffer);
        self.begin_session(Begin {
            brief: Some(text),
            ..begin
        });
        true
    }

    /// `brief-cancel` in a native session's `*Brief*`: nothing starts. Whether `buffer` was
    /// one.
    pub(in crate::app) fn cancel_first_message(&mut self, buffer: BufferId) -> bool {
        if self.hosting.natives.briefs.remove(&buffer).is_none() {
            return false;
        }
        self.editor.kill_buffer(buffer);
        self.editor.message("Session not started");
        true
    }
}
