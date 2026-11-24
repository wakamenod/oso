//! A native session's side of its switches (ADR 0165, part 4d): what `session-switch-mode`'s
//! rules are given of it, its process ended for a switch to the TUI, its transcript set aside
//! while the TUI runs its conversation (part 4d-1), and taken back when it comes back
//! natively (part 4d-2).
//!
//! Must not depend on: key presses or how a frame is drawn.
//!
//! - The transcript is kept as it was, rows, composer and all, under a new name,
//!   `*native: NAME*` (the author's Q4-5 (a)), so that the TUI's terminal takes
//!   `*claude: NAME*`, the name ADR 0121's lists, keys and agent tools find a session by. It
//!   is read-only and withheld from agents, as every native transcript is, and runs nothing:
//!   its keys and its composer's, folding and moves among them, only say that the session
//!   runs in the TUI. It is read as text: search, scroll and copy.
//! - It is not carried over an `oso-restart` nor brought back at oso's next start, as no
//!   native transcript's rows are (part 4b): the session comes back as a stopped TUI one.
//! - The session is no longer a native one: the record says `tui`, and the table's buffer is
//!   the terminal's.
//! - The TUI starts with what the session held (the author's Q4-7 (a)): the permission mode
//!   its next process would have started in (part 3b), and the model and effort chosen
//!   (part 3c).
//! - A transcript set aside goes when its buffer is killed, or when its session is gone from
//!   the table.

use oso_core::buffer::BufferId;

use super::note::NativeState;
use super::{Native, draw};
use crate::app::App;
use crate::sessions::hosted::Run;
use crate::sessions::native::switch::NativeAt;
use crate::sessions::start::Chosen;
use crate::sessions::worker::SessionKey;

/// What a transcript set aside says when a key asks it for anything.
const ASIDE: &str = "This session runs in the TUI now: its transcript sends nothing";

/// The name a transcript set aside takes.
pub(in crate::app::sessions::hosting) fn aside_name(name: &str) -> String {
    format!("*native: {name}*")
}

impl App {
    /// Where native session `key` is, for `session-switch-mode`'s rules.
    pub(in crate::app::sessions::hosting) fn native_switch_at(
        &self,
        key: SessionKey,
    ) -> Option<NativeAt> {
        let native = self.hosting.natives.map.get(&key)?;
        Some(NativeAt {
            runs: native.stream.is_some(),
            starting: native.starting(),
            turn: native.composing.busy,
            card: self.native_asks_person(key),
            interrupting: native.interrupt.interrupting(),
            mode_changing: native.mode.switching(),
            queued: native.composing.queued(),
            spoke: native.spoke,
            moved: native.moved,
            held: self.native_resume_held(key),
        })
    }

    /// Session `key`, back from the TUI (part 4d-2), takes its transcript set aside back,
    /// named `*claude: NAME*` again, stopped: whether there was one.
    pub(in crate::app::sessions::hosting) fn take_back_aside(
        &mut self,
        key: SessionKey,
        name: &str,
    ) -> bool {
        let Some(mut native) = self.hosting.natives.aside.remove(&key) else {
            return false;
        };
        // The TUI's turns follow its rows (part 5b).
        native.may_have_gone_on = true;
        let buffer = native.buffer;
        // The mode it resumes in, as held from before the hand-off: what the TUI changed is
        // not read back.
        let change = native.rows.note(&match native.mode.carried() {
            Some(mode) => format!("Back from the TUI, in mode {}.", mode.name()),
            None => "Back from the TUI.".to_owned(),
        });
        draw(&mut self.editor, &mut native, [change]);
        self.editor
            .rename_terminal_buffer(buffer, &super::super::buffer_name(name));
        self.hosting.natives.map.insert(key, native);
        self.mark_session_buffer(key, buffer);
        if let Some(session) = self.hosting.table.get_mut(key) {
            session.buffer = Some(buffer);
            session.run = Run::Stopped;
        }
        self.set_native_state(key, NativeState::Stopped);
        true
    }

    /// Ends native session `key`'s process for its switch to the TUI, which starts once it
    /// ended and was reaped (`native_ended`).
    pub(in crate::app::sessions::hosting) fn end_native_for_tui(&mut self, key: SessionKey) {
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        super::remote::stop(native);
        let change = native
            .rows
            .note("Handing the session to the TUI: claude ends, then the TUI resumes it.");
        draw(&mut self.editor, native, [change]);
    }

    /// Native session `key`'s switch to the TUI was given up once its process ended: it says
    /// so, in its transcript and the echo area.
    pub(in crate::app::sessions::hosting) fn native_switch_given_up(&mut self, key: SessionKey) {
        const GIVEN_UP: &str = "The TUI did not start: claude ended, but it was stopped, or an \
                                exit or a restart was asked meanwhile, or its end was not seen \
                                whole";
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        let change = native.rows.note(GIVEN_UP);
        draw(&mut self.editor, native, [change]);
        self.editor.message(GIVEN_UP);
    }

    /// Sets native session `key`'s transcript aside, with no process, as its session is
    /// handed to the TUI: renamed `*native: NAME*`, saying so, and out of the native sessions.
    /// What it held, for the TUI's command line; `None` if it is not a native session.
    pub(in crate::app::sessions::hosting) fn set_native_aside(
        &mut self,
        key: SessionKey,
    ) -> Option<Chosen> {
        let name = self.hosting.table.get(key)?.name.clone();
        let mut native = self.hosting.natives.map.remove(&key)?;
        let held = chosen_of(&native);
        // Nothing of it asks, runs or is shown as running any more.
        native.stream = None;
        native.control = None;
        self.editor.set_running(native.buffer, false);
        let change = native.rows.note(&format!(
            "In the TUI since here, in {}.",
            super::super::buffer_name(&name)
        ));
        draw(&mut self.editor, &mut native, [change]);
        self.editor
            .rename_terminal_buffer(native.buffer, &aside_name(&name));
        self.editor
            .set_mode_note(native.buffer, "in the TUI".to_owned());
        self.editor.set_footer(native.buffer, None);
        // One set aside before, whose session came back natively in a buffer of its own,
        // goes: a session keeps one transcript aside.
        if let Some(old) = self.hosting.natives.aside.insert(key, native) {
            self.kill_native_buffers(&old);
        }
        Some(held)
    }

    /// Whether `buffer` is a transcript set aside, or its composer: then the echo area says
    /// so, and its key does nothing.
    pub(in crate::app) fn refuse_aside_key(&mut self, buffer: BufferId) -> bool {
        let aside = (self.hosting.natives.aside.values())
            .any(|native| native.buffer == buffer || native.composing.buffer == Some(buffer));
        if aside {
            self.editor.message(ASIDE);
        }
        aside
    }

    /// The session whose transcript set aside, or its composer, is `buffer`.
    pub(in crate::app::sessions::hosting) fn aside_session(
        &self,
        buffer: BufferId,
    ) -> Option<SessionKey> {
        (self.hosting.natives.aside.iter()).find_map(|(key, native)| {
            (native.buffer == buffer || native.composing.buffer == Some(buffer)).then_some(*key)
        })
    }

    /// After each event: each transcript set aside whose buffer was killed, or whose session
    /// is gone from the table, goes with its composer and strip.
    pub(super) fn sweep_aside(&mut self) {
        if self.hosting.natives.aside.is_empty() {
            return;
        }
        let (editor, table) = (&self.editor, &self.hosting.table);
        let gone: Vec<SessionKey> = (self.hosting.natives.aside.iter())
            .filter(|(key, native)| {
                editor.buffer(native.buffer).is_none() || table.get(**key).is_none()
            })
            .map(|(key, _)| *key)
            .collect();
        for key in gone {
            if self.hosting.table.get(key).is_none() {
                self.hosting.natives.switches.forget(key);
            }
            if let Some(native) = self.hosting.natives.aside.remove(&key) {
                self.kill_native_buffers(&native);
            }
        }
    }
}

/// What `native` held for the TUI it is handed to: the mode its next process would start in,
/// and the model and effort chosen.
fn chosen_of(native: &Native) -> Chosen {
    let (model, effort) = native.choice.for_start();
    Chosen {
        permission_mode: native.mode.carried(),
        model,
        effort,
    }
}
