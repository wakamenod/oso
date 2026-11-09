//! What a buffer knows of the agent channel (ADR 0085): whether the channel opened it, so
//! that the channel saves it only once the person agrees, and whether a path's name says
//! the file may hold secrets, which the channel refuses to open.
//!
//! Must not depend on: the editor or the host's channel.

use std::path::Path;

use super::{Buffer, Verdict};

impl Buffer {
    /// Whether `path`, as it is spelled, names a file that may hold secrets, by the patterns
    /// every buffer is judged by (ADR 0044). Only a name: where a link leads is the loader's
    /// to find.
    pub fn name_may_hold_secrets(path: &Path) -> bool {
        crate::secret::judge_name(path) == Verdict::SecretName
    }

    /// Notes that the agent channel opened it.
    pub fn mark_opened_by_agent(&mut self) {
        self.opened_by_agent = true;
    }

    /// Whether the agent channel opened it, so that the channel saves it only once the
    /// person agrees.
    pub fn opened_by_agent(&self) -> bool {
        self.opened_by_agent
    }

    /// Notes whether the agent channel is still opening it: loading it, or not yet past the
    /// channel's checks of what it may show.
    pub fn set_agent_pending(&mut self, pending: bool) {
        self.agent_pending = pending;
    }

    /// Whether the agent channel is still opening it, so that nothing shows it or takes it for
    /// the buffer of its file before the channel's checks (ADR 0087).
    pub fn agent_pending(&self) -> bool {
        self.agent_pending
    }
}

#[cfg(test)]
mod tests {
    use super::Buffer;
    use std::path::Path;

    #[test]
    fn a_name_alone_can_say_a_file_may_hold_secrets() {
        assert!(Buffer::name_may_hold_secrets(Path::new("/home/a/.env")));
        assert!(Buffer::name_may_hold_secrets(Path::new(
            "/home/a/.ssh/config"
        )));
        assert!(!Buffer::name_may_hold_secrets(Path::new(
            "/home/a/notes.md"
        )));
    }

    #[test]
    fn a_buffer_the_agent_opened_says_so() {
        let mut buffer = Buffer::from_text("notes.md", "");
        assert!(!buffer.opened_by_agent());
        buffer.mark_opened_by_agent();
        assert!(buffer.opened_by_agent());
    }
}
