//! A TUI session taken back natively (ADR 0165, part 4d-2): `session-switch-mode` in its
//! terminal, and a TUI a hand-off started that exits cleanly of itself (the author's Q4-6 (a)).
//! Where each switch is, the hand-off's TUIs among it, is `Switches`' alone.
//!
//! Must not depend on: key presses or how a frame is drawn; the TUI's `claude` is waited for
//! on the native resume's start thread.
//!
//! - Only where a native session may run: its project's root trusted now, by an open buffer
//!   of it (its terminal is one) or the record of trusted projects (part 4a), or in an issue
//!   space's own directory its space's gate (ADR 0176), judged before anything changes and
//!   again once the TUI ended; else it says why, and the session stays in the TUI. Only the
//!   resume itself, its terminal gone, may be judged by its worktree's claim.
//! - A running TUI is hung up as oso's stop hangs a terminal up, but not noted as a stop: a
//!   stop made meanwhile, or a resume held (a stop asked, oso waiting to quit or restarting,
//!   an orchestrator's replacement), gives the switch up, and the session stays a stopped TUI
//!   one.
//! - Once the terminal said its program ended, after the event (its end is said as its handle
//!   is dropped), its terminal goes and the session resumes natively in the transcript set
//!   aside, which takes `*claude: NAME*` back, or after an oso restart a new one, in each window
//!   that showed the terminal (the one set aside before the terminal goes). Every native
//!   resume of a session whose TUI's `claude` may still run waits for it to be gone on its
//!   start thread (`sessions::start::gone`), whatever stopped the TUI. The record says native
//!   once the native process started: a resume refused leaves it a TUI session there.

use std::path::Path;

use oso_term::End;

use crate::app::App;
use crate::sessions::hosted::{Run, TuiProcess};
use crate::sessions::identity::SessionId;
use crate::sessions::native::switch::{Then, TuiAt};
use crate::sessions::start::trust::NotTrusted;
use crate::sessions::worker::SessionKey;

impl App {
    /// Takes TUI session `key`, `name`, back natively: its `claude` is hung up first if it
    /// runs.
    pub(super) fn tui_hand_to_native(&mut self, key: SessionKey, name: &str) {
        let Some(session) = self.hosting.table.get(key) else {
            return;
        };
        let pid = match session.run {
            Run::Running { child } => child,
            _ => None,
        };
        let at = TuiAt {
            runs: session.runs(),
            starting: session.run == Run::Starting,
            state: crate::app::sessions::worktrees::hosted_state(session, &self.watch),
            pid,
            held: self.native_resume_held(key),
        };
        let (id, directory, root) = (
            session.id.clone(),
            session.directory.clone(),
            session.root.clone(),
        );
        if let Err(why) = self.switch_trust(name, &id, &directory, root.as_deref()) {
            return self.editor.message(why);
        }
        match self.hosting.natives.switches.hand_to_native(key, &at) {
            Err(refusal) => self.editor.message(refusal.message(name)),
            Ok(Then::End) if self.hang_up_terminal(key) => {
                self.keep_tui_process(key, pid);
                self.editor.message(format!(
                    "Session {name} comes back natively once its TUI's claude ended"
                ));
            }
            // Its terminal went meanwhile: nothing ends, and the switch is over.
            Ok(Then::End) => {
                self.hosting.natives.switches.over(key);
                self.editor
                    .message(format!("Session {name}'s TUI does not run"));
            }
            // Due after this event.
            Ok(Then::Start) => {}
        }
    }

    /// Session `key`'s TUI `claude`, process `pid`, hung up for a switch, is kept to wait for
    /// if its start did not record one, as for a TUI carried over an `oso-restart`: with no
    /// start time, any process with the id counts as it.
    fn keep_tui_process(&mut self, key: SessionKey, pid: Option<u32>) {
        let Some(session) = self.hosting.table.get_mut(key) else {
            return;
        };
        if session.tui_process.is_none() {
            session.tui_process = pid.map(|pid| TuiProcess { pid, started: None });
        }
    }

    /// Why session `name`, conversation `id`, in `directory` of the project at `root`, does
    /// not come back natively, if it does not: its project not trusted now for a native
    /// session, or in a space's directory, its space's gate refused (ADR 0176).
    fn switch_trust(
        &self,
        name: &str,
        id: &SessionId,
        directory: &Path,
        root: Option<&Path>,
    ) -> Result<(), String> {
        if let Some(gate) = self.space_gate_now(id, directory) {
            return gate
                .map_err(|why| format!("Session {name} stays in the TUI, as it {}", why.reason()));
        }
        let why = match self.native_trust_listed(directory, root, false) {
            Ok(_) => return Ok(()),
            Err(NotTrusted::NotOpen) => {
                "its project is not open, and the record of trusted \
                                         projects does not hold it: open a file of it first"
            }
            Err(NotTrusted::Untrusted) => "its project is not trusted now",
            Err(NotTrusted::NotAtRoot) => "it runs below its project's root",
            Err(NotTrusted::NoRecord) => "the record of trusted projects is not read",
        };
        Err(format!(
            "Session {name} stays in the TUI, as a native session runs only at a trusted \
             project's root: {why}"
        ))
    }

    /// The terminal of TUI session `key` said its program ended as `end`, a stop of oso's or
    /// the person's if `stopped`: whether the session comes back natively, after the event,
    /// for a switch that hung it up, or as a TUI a hand-off started that exited cleanly of
    /// itself. A switch given up says so, and the session stays a stopped TUI one.
    pub(in crate::app::sessions::hosting) fn tui_end_comes_back(
        &mut self,
        key: SessionKey,
        end: &End,
        stopped: bool,
    ) -> bool {
        let held = self.native_resume_held(key);
        let switches = &mut self.hosting.natives.switches;
        if switches.ending(key).is_some() {
            if stopped || held {
                switches.over(key);
                self.editor.message(
                    "The session was not taken back natively: it was stopped, or an exit or a \
                     restart was asked, while its TUI ended",
                );
                return false;
            }
            return switches.ended(key).is_some();
        }
        let clean = matches!(end, End::Exited(status) if status.success()) && !stopped && !held;
        switches.tui_exited(key, clean)
    }

    /// After each event, once the terminals' ends were said: each session whose switch back
    /// is due is native again and resumes; its TUI's terminal goes.
    pub(in crate::app) fn take_back_due(&mut self) {
        for key in self.hosting.natives.switches.take_due() {
            self.take_back_natively(key);
        }
    }

    /// TUI session `key`, its `claude` ended, is native again and resumes, its start thread
    /// waiting for its TUI's `claude` to be gone. Refused, it says why: a native resume refused
    /// on its start thread leaves it stopped in its transcript.
    fn take_back_natively(&mut self, key: SessionKey) {
        let Some(session) = self.hosting.table.get(key) else {
            self.hosting.natives.switches.over(key);
            return;
        };
        let (name, id, directory, root, terminal) = (
            session.name.clone(),
            session.id.clone(),
            session.directory.clone(),
            session.root.clone(),
            session.buffer,
        );
        // Judged again now: a project no longer trusted keeps its session in the TUI.
        if let Err(why) = self.switch_trust(&name, &id, &directory, root.as_deref()) {
            self.hosting.natives.switches.over(key);
            return self.editor.message(why);
        }
        // Its TUI's terminal, whose program ended, goes; it no longer holds the session. The
        // windows that showed it show the transcript instead: the one set aside before the
        // terminal goes, so that no window passes through what a kill would show.
        let windows = match terminal {
            Some(terminal) => super::tree_windows_showing(&self.editor, terminal),
            None => Vec::new(),
        };
        if let Some(aside) = self.hosting.natives.aside_buffer(key) {
            for &id in &windows {
                self.editor.show_in(id, aside);
            }
        }
        if let Some(session) = self.hosting.table.get_mut(key) {
            session.buffer = None;
        }
        if let Some(terminal) = terminal {
            self.editor.kill_buffer(terminal);
        }
        if !self.take_back_aside(key, &name) {
            // After a restart, no transcript was set aside: a new one, shown once it is there.
            self.bring_back_native(key);
            self.show_session_in(key, &windows);
        }
        if let Err(why) = self.resume_hosted(key, true) {
            self.hosting.natives.switches.over(key);
            self.editor
                .message(format!("Session {name} did not resume natively: {why}"));
        }
    }
}
