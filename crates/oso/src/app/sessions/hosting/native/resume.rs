//! A native session's resume and fork (ADR 0165, step 2, R1 and the author's Q2 (a)): a
//! stopped native session resumes natively, in its own buffer, keeping its rows with one
//! "Resumed" row; a fork of a session in a trusted project under `mode = "native"` is native, in
//! a new buffer. Neither opens `*Brief*`: the session starts at once, and the person writes in
//! its composer.
//!
//! Must not depend on: key presses or how a frame is drawn; the start thread starts the process.
//!
//! - Its project's trust is looked at again before the spawn: an open project at its root says
//!   it now, else the trust the session started with while oso runs. With none, the resume
//!   refuses and points to `session-fork`, which then runs in a terminal.
//! - A resume takes no baseline: the session keeps the one it had.

use std::path::Path;

use super::note::NativeState;
use crate::app::App;
use crate::git::TrustedRoot;
use crate::sessions::hosted::Run;
use crate::sessions::start::{Kind, NativeOrder, Order, Runs};
use crate::sessions::worker::SessionKey;

impl App {
    /// Resumes the stopped native session `key` natively, in its own buffer, shown and with
    /// the composer selected if `shown`.
    ///
    /// # Errors
    ///
    /// Why it does not resume: it has no buffer, its project is not trusted, or its space's
    /// checks fail.
    pub(in crate::app) fn resume_native(
        &mut self,
        key: SessionKey,
        shown: bool,
    ) -> Result<(), String> {
        let Some(session) = self.hosting.table.get(key) else {
            return Err("That session is gone".into());
        };
        let (id, name, directory, root) = (
            session.id.clone(),
            session.name.clone(),
            session.directory.clone(),
            session.root.clone(),
        );
        let Some(buffer) = (self.hosting.natives.map.get(&key))
            .map(|native| native.buffer)
            .filter(|id| self.editor.buffer(*id).is_some())
        else {
            return Err(format!("Session {name} has no buffer to resume in"));
        };
        let Some(trusted) = self.native_trust(key, root.as_deref().unwrap_or(&directory)) else {
            return Err(format!(
                "Session {name}'s project is not trusted: it does not resume natively; \
                 session-fork continues it in a terminal"
            ));
        };
        let space = self.orchestrator_start_for(key)?;
        self.turn_agent_channel_on_for_a_session();
        if let Some(session) = self.hosting.table.get_mut(key) {
            session.run = Run::Starting;
        }
        let Some(order) = self.resume_native_buffer(key) else {
            return Err(format!("Session {name} has no buffer to resume in"));
        };
        if shown {
            self.editor.show(buffer);
            self.composer_go(key, true);
        }
        self.order_session(Order {
            token: key,
            kind: Kind::Resume(id),
            name,
            directory,
            project: root,
            trusted: Some(trusted),
            state: crate::app::reviews::state(),
            claude: self.configuration.session_claude().map(Path::to_path_buf),
            runs: Runs::Native(order),
            brief: None,
            steer_worktrees: self.configuration.steer_worktrees(),
            check_shared: false,
            space,
        });
        Ok(())
    }

    /// Native session `key`'s buffer made ready to run again: a "Resumed" row, no turn under
    /// way, as no message is written yet. The start thread's order.
    fn resume_native_buffer(&mut self, key: SessionKey) -> Option<NativeOrder> {
        // Its bridge asks the person's permission there (ADR 0165, step 2).
        self.start_sessions_socket();
        let native = self.hosting.natives.map.get_mut(&key)?;
        native.first = false;
        native.ended = None;
        native.resumed = true;
        native.composing.busy = false;
        let change = native.rows.note("Resumed.");
        super::draw(&mut self.editor, native, [change]);
        let buffer = native.buffer;
        self.editor.set_running(buffer, true);
        self.set_native_state(key, NativeState::Starting);
        self.hosting.starting.insert(key, buffer);
        Some(self.native_order(key, buffer))
    }

    /// A refused start of native session `key`: a resume's buffer stays, stopped again, and
    /// says so. Whether it was a resume.
    pub(super) fn native_resume_refused(&mut self, key: SessionKey) -> bool {
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return false;
        };
        if !native.resumed {
            return false;
        }
        native.resumed = false;
        let buffer = native.buffer;
        let change = native.rows.note("It did not resume.");
        super::draw(&mut self.editor, native, [change]);
        self.editor.set_running(buffer, false);
        self.set_native_state(key, NativeState::Stopped);
        self.composer_ended(key);
        true
    }

    /// A fork's runs, natively in a new buffer when the project at `root` is trusted under
    /// `mode = "native"`, with its trust; `None` for a terminal.
    pub(in crate::app) fn native_fork(
        &mut self,
        key: SessionKey,
        name: &str,
        directory: &Path,
        root: Option<&Path>,
    ) -> Option<(NativeOrder, TrustedRoot)> {
        if !self.configuration.native_sessions() {
            return None;
        }
        let trusted = self.open_root_trust(root.unwrap_or(directory)).flatten()?;
        let order = self.native_session_buffer(key, name, directory.to_path_buf(), true, None);
        self.composer_go(key, true);
        Some((order, trusted))
    }

    /// The trust of native session `key`'s project at `root` now: an open project's whose root
    /// it is, looked up now so that the person's later trust or distrust counts, else the one
    /// the session started with, unless taken back.
    fn native_trust(&self, key: SessionKey, root: &Path) -> Option<TrustedRoot> {
        match self.open_root_trust(root) {
            Some(now) => now,
            None => (self.hosting.trusts.get(&key))
                .filter(|trusted| !trusted.revoked())
                .cloned(),
        }
    }
}
