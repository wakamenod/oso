//! Remote Control at a native session's start, as Claude Code's settings say (ADR 0188): what the
//! configuration's thread read of them, and the leave a start is given by them. The start thread
//! reads them again (`sessions::start::connect`).
//!
//! Must not depend on: key presses or the files: the configuration's thread reads them.

use crate::app::App;
use crate::sessions::start::connect::Connect;
use crate::sessions::start::connect::at_startup::{Files, Reading};
use crate::sessions::worker::SessionKey;

/// What a session is told the first time Claude Code's settings connect it (ADR 0188).
pub(super) const TOLD: &str = "Remote Control connects this session to claude.ai, as \
     remoteControlAtStartup in your Claude Code settings says: its conversation and tool output, \
     earlier ones too, its name, git remotes, branch and directory are sent there and kept, and \
     whoever holds the account can drive it. native-remote-control turns it off for this session.";

impl App {
    /// What Claude Code's settings say of connecting each session at start, as the
    /// configuration's thread read them: an unread file is said in `*Messages*` (ADR 0188).
    pub(in crate::app) fn remote_at_startup_read(&mut self, reading: Reading) {
        if let Reading::Unread(why) = &reading {
            self.editor.note(format!(
                "Remote Control does not connect native sessions at start: {why}"
            ));
        }
        self.hosting.natives.remotes.at_startup = reading;
    }

    /// Leave for native session `key`'s next start to connect as Claude Code's settings say, if
    /// they do and it may: one turned off is not, nor one whose transcript or composer holds
    /// secret text, which its start would send.
    pub(in crate::app::sessions::hosting::native) fn remote_at_startup(
        &self,
        key: SessionKey,
    ) -> Option<Connect> {
        let remotes = &self.hosting.natives.remotes;
        if !remotes.at_startup.on() || remotes.turned_off.contains(&key) {
            return None;
        }
        let native = self.hosting.natives.map.get(&key)?;
        let secret = |id| (self.editor.buffer(id)).is_some_and(oso_core::buffer::Buffer::is_secret);
        if secret(native.buffer) || native.composing.buffer.is_some_and(secret) {
            return None;
        }
        Some(Connect::at_startup(Files::person()))
    }

    /// Whether Claude Code's settings connect each native session at start.
    pub(in crate::app::sessions::hosting) fn remote_at_startup_on(&self) -> bool {
        self.hosting.natives.remotes.at_startup.on()
    }
}
