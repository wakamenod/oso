//! Remote Control at a native session's start, as Claude Code's settings say (ADR 0189): what the
//! configuration's thread read of them, and the leave a start is given by them. The start thread
//! reads them again (`sessions::start::connect`).
//!
//! Must not depend on: key presses or the files: the configuration's thread reads them.

use crate::app::App;
use crate::sessions::identity::SessionId;
use crate::sessions::start::connect::Connect;
use crate::sessions::start::connect::at_startup::{Files, Reading};
use crate::sessions::worker::SessionKey;

/// What a session is told the first time Claude Code's settings connect it (ADR 0189).
pub(super) const TOLD: &str = "Remote Control connects this session to claude.ai, as \
     remoteControlAtStartup in your Claude Code settings says: its conversation and tool output, \
     earlier ones too, its name, git remotes, branch and directory are sent there and kept, and \
     whoever holds the account can drive it. native-remote-control turns it off for this session.";

impl App {
    /// What Claude Code's settings say of connecting each session at start, as the
    /// configuration's thread read them: an unread file is said in `*Messages*` (ADR 0189).
    pub(in crate::app) fn remote_at_startup_read(&mut self, reading: Reading) {
        if let Reading::Unread(why) = &reading {
            self.editor.note(format!(
                "Remote Control does not connect native sessions at start: {why}"
            ));
        }
        self.hosting.natives.remotes.at_startup = reading;
    }

    /// Leave for native session `key`'s next start to connect as Claude Code's settings say, if
    /// they do and it may.
    pub(in crate::app::sessions::hosting::native) fn remote_at_startup(
        &self,
        key: SessionKey,
    ) -> Option<Connect> {
        let on = self.hosting.natives.remotes.at_startup.on();
        (on && self.remote_at_startup_holds(key)).then(|| Connect::at_startup(Files::person()))
    }

    /// Whether native session `key` may be connected as Claude Code's settings say: not one
    /// turned off, nor one whose transcript or composer holds secret text, which its start would
    /// send; as it is ordered, as a held order goes, and before it is asked to connect.
    pub(in crate::app::sessions::hosting) fn remote_at_startup_holds(
        &self,
        key: SessionKey,
    ) -> bool {
        let remotes = &self.hosting.natives.remotes;
        let id = self.hosting.table.get(key).map(|session| &session.id);
        let carried = |id: &SessionId| remotes.all_carried_off || remotes.carried_off.contains(id);
        if remotes.turned_off.contains(&key) || id.is_some_and(carried) {
            return false;
        }
        let Some(native) = self.hosting.natives.map.get(&key) else {
            return false;
        };
        let secret = |id| (self.editor.buffer(id)).is_some_and(oso_core::buffer::Buffer::is_secret);
        !secret(native.buffer) && !native.composing.buffer.is_some_and(secret)
    }

    /// Native session `key`, connected as Claude Code's settings say, turned secret since it was
    /// ordered, as its earlier turns came: it is not asked to connect, and is turned off, so that
    /// its process ends and resumes blocked. Whether it was.
    pub(in crate::app::sessions::hosting::native) fn remote_at_startup_secret(
        &mut self,
        key: SessionKey,
    ) -> bool {
        let at_startup = (self.hosting.natives.map.get(&key))
            .is_some_and(|native| native.remote_control.at_startup && !native.remote_control.off());
        if !at_startup || self.remote_at_startup_holds(key) {
            return false;
        }
        self.remote_stays_off(key, "its transcript or composer holds secret text");
        self.remote_off(key);
        true
    }

    /// The fork `key` of the conversation `from`: turned off where that is (ADR 0189).
    pub(in crate::app::sessions::hosting) fn remote_off_for_a_fork_of(
        &mut self,
        from: &SessionId,
        key: SessionKey,
    ) {
        let remotes = &self.hosting.natives.remotes;
        let source = (self.hosting.table.iter()).find(|session| session.id == *from);
        let off = remotes.all_carried_off
            || remotes.carried_off.contains(from)
            || source.is_some_and(|source| remotes.turned_off.contains(&source.key));
        if off {
            self.hosting.natives.remotes.turned_off.insert(key);
        }
    }

    /// The conversations turned off, for a restart's state: running or not.
    pub(in crate::app) fn remote_turned_off(&self) -> Vec<String> {
        let remotes = &self.hosting.natives.remotes;
        let mut ids: Vec<String> = (self.hosting.table.iter())
            .filter(|session| {
                remotes.all_carried_off
                    || remotes.turned_off.contains(&session.key)
                    || remotes.carried_off.contains(&session.id)
            })
            .map(|session| session.id.as_str().to_owned())
            .collect();
        ids.sort();
        ids.dedup();
        ids
    }

    /// The instance before turned off the conversations `ids`, or all of them.
    pub(in crate::app) fn remote_turned_off_carried(&mut self, ids: Result<Vec<String>, ()>) {
        let remotes = &mut self.hosting.natives.remotes;
        match ids {
            Ok(ids) => remotes
                .carried_off
                .extend(ids.iter().filter_map(|id| SessionId::parse(id))),
            Err(()) => remotes.all_carried_off = true,
        }
    }

    /// Whether Claude Code's settings connect each native session at start.
    pub(in crate::app::sessions::hosting) fn remote_at_startup_on(&self) -> bool {
        self.hosting.natives.remotes.at_startup.on()
    }
}
