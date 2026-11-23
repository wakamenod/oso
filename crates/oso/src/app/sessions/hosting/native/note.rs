//! A native session's run state and its modeline note (ADR 0165): `starting`, nothing while
//! it runs, or `interrupting` (step 2c), `stopped` or `failed`, how many of its messages
//! wait (step 2), then its status line (part 3a, `status`), each part once known, joined by
//! ` · `. The note is written here alone; a narrow modeline drops its parts from the right.
//!
//! Must not depend on: key presses or how a frame is drawn.

use crate::app::App;
use crate::sessions::native::status::SEPARATOR;
use crate::sessions::worker::SessionKey;

/// Where a native session's process is, as its modeline says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NativeState {
    /// Its process is being started.
    Starting,
    Running,
    /// It ended, or came back from oso's last run with no process.
    Stopped,
    /// It ended with an error.
    Failed,
}

impl NativeState {
    fn said(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Running => "",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
        }
    }
}

impl App {
    /// Sets native session `key`'s run state, and its modeline's note.
    pub(super) fn set_native_state(&mut self, key: SessionKey, state: NativeState) {
        if let Some(native) = self.hosting.natives.map.get_mut(&key) {
            native.state = state;
        }
        self.write_native_note(key);
    }

    /// Native session `key`'s modeline note: its run's state, how many wait, and its status
    /// line. Returns whether it changed.
    pub(super) fn write_native_note(&mut self, key: SessionKey) -> bool {
        let Some(native) = self.hosting.natives.map.get(&key) else {
            return false;
        };
        let interrupting = native.interrupt.interrupting();
        let state = match native.state {
            NativeState::Running if interrupting => "interrupting",
            state => state.said(),
        };
        let mut note = match (state, native.composing.queued()) {
            (state, 0) => state.to_owned(),
            ("", count) => format!("{count} queued"),
            (state, count) => format!("{state}{SEPARATOR}{count} queued"),
        };
        native.status.line.write(&mut note);
        let buffer = native.buffer;
        let same = (self.editor.buffer(buffer)).is_some_and(|b| b.mode_note() == note);
        if !same {
            self.editor.set_mode_note(buffer, note);
        }
        !same
    }
}
