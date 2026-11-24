//! A native session's run state and its modeline note (ADR 0165): `starting`, nothing while
//! it runs, or `interrupting` (step 2c), `stopped` or `failed`, how many of its messages
//! wait (step 2), then its status line (part 3a, `status`), each part once known, joined by
//! ` · `. The note is written here alone; a narrow modeline drops its parts from the right.
//!
//! Must not depend on: key presses or how a frame is drawn.

use crate::app::App;
use crate::sessions::hosted::Run;
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
    /// Where native session `key`'s process is, as the session table names it, for what
    /// waits for it to run, as a review's question does (ADR 0176, part O-2): `None` for a
    /// session that is not a native one.
    pub(in crate::app) fn native_run(&self, key: SessionKey) -> Option<Run> {
        let native = self.hosting.natives.map.get(&key)?;
        Some(match (&native.stream, native.state) {
            (Some(stream), _) => Run::Running {
                child: stream.pid(),
            },
            (None, NativeState::Starting) => Run::Starting,
            (None, NativeState::Failed) => Run::Failed,
            (None, _) => Run::Stopped,
        })
    }

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
        // Remote Control's mark first, as a narrow modeline drops its parts from the right
        // (ADR 0181).
        let queued = match native.composing.queued() {
            0 => String::new(),
            count => format!("{count} queued"),
        };
        let mark = native.remote.mark().unwrap_or_default();
        let mut note = [mark, state, &queued]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(SEPARATOR);
        let effort = native.choice.effort();
        native
            .status
            .line
            .write(native.mode.shown(), effort, &mut note);
        let buffer = native.buffer;
        let same = (self.editor.buffer(buffer)).is_some_and(|b| b.mode_note() == note);
        if !same {
            self.editor.set_mode_note(buffer, note);
        }
        !same
    }
}
