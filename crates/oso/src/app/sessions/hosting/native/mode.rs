//! A native session's permission mode in the app (ADR 0165, Q4; part 3b): `S-TAB` in its
//! transcript and its composer switches it to the next, and the modeline shows the mode the
//! CLI says. The rules are `sessions::native::mode`'s: this carries out what they come to, at
//! the times they set; its answers and deadlines come through `control`.
//!
//! Must not depend on: key presses or how a frame is drawn.
//!
//! - On tier 2 the switch is `set_permission_mode`, and the process stays; on tier 1, or when
//!   the request fell back, the idle process is stopped and resumed in the new mode (`end`), as
//!   an interrupt's resume is, under the trust judged then. During a turn that is refused, and
//!   so it is while a resume could not start: a stop asked, oso waiting to quit or restart, an
//!   orchestrator being replaced, or a conversation that moved.
//! - Each new process of the session is given the mode it held (`native_order`), as `--resume`
//!   does not keep one. A session that comes back after oso's restart has none, and starts in
//!   the mode the person's Claude Code settings give it (the author's Q-B2).
//! - Only the person switches: the command is `Person`, and no agent or channel method sets a
//!   mode. `bypassPermissions` and `dontAsk` are never sent.
//! - Why a request fell back is logged as the CLI said it, escaped.

use std::time::Instant;

use super::Native;
use crate::app::App;
use crate::sessions::native::mode::{Process, Refusal, Then};
use crate::sessions::native::wire::Event;
use crate::sessions::native::wire::control::{Answer, Request};
use crate::sessions::worker::SessionKey;

/// `initialize` was answered, as `answer` says: the mode the session is in, if it succeeded.
/// Returns whether the mode shown changed.
pub(super) fn initialize_answered(native: &mut Native, answer: &mut Answer) -> bool {
    match answer.mode.take() {
        Some(mode) if answer.error.is_none() => native.mode.said(&mode, native.remote.watched()),
        _ => false,
    }
}

/// Notes the mode `event` says of `native`, if it says one. Returns whether the mode shown
/// changed. One that may be from elsewhere is never carried (ADR 0181).
pub(super) fn mode_event(native: &mut Native, event: &Event) -> bool {
    native.mode.event(event, native.remote.watched())
}

/// Where native session `native`'s process is, for the mode's rules, `resumable` if no resume
/// of it is held now.
pub(super) fn process(native: &Native, resumable: bool) -> Process {
    Process {
        idle: !native.composing.busy && !native.interrupt.interrupting(),
        spoke: native.spoke,
        resumable,
    }
}

impl App {
    /// Where native session `key`'s process is, for the mode's rules.
    pub(super) fn mode_process(&self, key: SessionKey) -> Option<Process> {
        let native = self.hosting.natives.map.get(&key)?;
        Some(process(native, !self.native_resume_held(key)))
    }

    /// `native-session-cycle-mode`: switches native session `key` to its next mode.
    pub(super) fn native_cycle_mode(&mut self, key: SessionKey) {
        let Some(process) = self.mode_process(key) else {
            return;
        };
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        // No request or stop goes before the start thread gave the stream.
        let control = match (&native.stream, native.control.as_mut()) {
            (Some(_), control) => control,
            (None, Some(_)) => return self.editor.message(Refusal::Starting.message()),
            (None, None) => None,
        };
        match native.mode.begin(control, process, Instant::now()) {
            Ok(then) => self.mode_step(key, then),
            Err(refusal) => self.editor.message(refusal.message()),
        }
    }

    /// Carries out `then` for native session `key`.
    pub(super) fn mode_step(&mut self, key: SessionKey, then: Then) {
        let (id, mode) = match then {
            Then::Log(why) => return log::info!("native session control: {why:?}"),
            Then::Refused(refusal) => return self.editor.message(refusal.message()),
            Then::Request(id, mode) => (Some(id), mode),
            Then::Respawn(mode) => (None, mode),
        };
        let process = self.mode_process(key);
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        let Some(stream) = &native.stream else {
            // Its process ended meanwhile: the switch is over.
            return native.mode.gave_up();
        };
        let changing = format!("Changing the mode to {}…", mode.name());
        let Some(id) = id else {
            // Idle: claude ends, and resumes in the mode (`end`).
            stream.stop();
            self.native_note_row(
                key,
                &format!(
                    "Changing the mode to {}: claude ends, and resumes its conversation in it.",
                    mode.name()
                ),
            );
            return self.editor.message(changing);
        };
        if stream.request(id, Request::SetPermissionMode(mode)).is_ok() {
            return self.editor.message(changing);
        }
        let (Some(control), Some(process)) = (native.control.as_mut(), process) else {
            return;
        };
        for then in native.mode.not_written(control, process) {
            self.mode_step(key, then);
        }
    }
}
