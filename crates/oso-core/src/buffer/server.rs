//! What the modeline says of the language server that follows a buffer (ADR 0138, step 3):
//! `lsp`, dim while it starts, with its progress, plain when ready, and `lsp!` when stopped.
//! The host sets it from what the server's thread reports; nothing here knows of servers.
//!
//! Must not depend on: `editor` or `window`; the modeline reads this.

use super::Buffer;

/// Where the server that follows a buffer is, as the modeline shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServerMark {
    /// Starting, or waiting to start again after a crash: `lsp`, dim.
    Starting,
    /// Running: `lsp`.
    Ready,
    /// Running and reporting work, done to this percentage if it says: `lsp 34%`.
    Working(Option<u8>),
    /// Stopped for the session: `lsp!`, in the warning face.
    Stopped,
}

impl Buffer {
    /// What the modeline says of its server, `None` while none follows it.
    pub fn server(&self) -> Option<ServerMark> {
        self.server
    }

    /// Sets what the modeline says of its server, as the host learns it.
    pub fn set_server(&mut self, mark: Option<ServerMark>) {
        self.server = mark;
    }
}
