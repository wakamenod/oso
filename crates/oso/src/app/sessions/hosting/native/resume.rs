//! A native session's resume and fork (ADR 0165, step 2, R1 and the author's Q2 (a)): a
//! stopped native session resumes natively, in its own buffer, keeping its rows with one
//! "Resumed" row; a fork of a session at a trusted project's root under `mode = "native"` is
//! native, in a new buffer. Each starts at once, and the person writes in its composer. `resume_hosted` and `fork_from` order them.
//!
//! Must not depend on: key presses or how a frame is drawn; the start thread starts the process.
//!
//! - Only at a project's root that an open buffer of it says is trusted now (`trust`), which
//!   the start thread looks at again before the spawn; else a resume refuses, saying why, and
//!   a fork runs in a terminal.
//! - A resume takes no baseline: the session keeps the one it had.
//! - A resume and a fork start in the mode the session was in (part 3b), as `--resume` keeps
//!   none: a fork, its source's.
//! - `claude -p --resume` writes no earlier turn on its stream (a probe of 2026-11-22): the
//!   rows kept are the transcript.

use std::path::Path;

use oso_core::buffer::BufferId;

use super::note::NativeState;
use crate::app::App;
use crate::app::sessions::hosting::trust::NotTrusted;
use crate::git::TrustedRoot;
use crate::sessions::native::mode::ModeState;
use crate::sessions::start::NativeOrder;
use crate::sessions::worker::SessionKey;

impl App {
    /// The trust native session `name`, in `directory` of the project at `root`, resumes under.
    ///
    /// # Errors
    ///
    /// Why it does not resume natively, and what does.
    pub(in crate::app::sessions::hosting) fn native_resume_trust(
        &self,
        name: &str,
        directory: &Path,
        root: Option<&Path>,
    ) -> Result<TrustedRoot, String> {
        let fork = "session-fork continues it in a terminal";
        self.native_trust_now(directory, root)
            .map_err(|why| match why {
                NotTrusted::NotOpen => format!(
                    "Session {name}'s project is not open: open a file of it to resume it \
                 natively, or {fork}"
                ),
                NotTrusted::Untrusted => {
                    format!("Session {name}'s project is not trusted now: {fork}")
                }
                NotTrusted::NotAtRoot => {
                    format!("Session {name} runs below its project's root: {fork}")
                }
            })
    }

    /// Native session `key`'s `buffer` made ready to run again: a "Resumed" row, no turn under
    /// way, as no message is written yet, and a queue that ended waiting for an empty
    /// `C-c C-c`; shown, with the composer selected, if `shown`. The start thread's order.
    pub(in crate::app::sessions::hosting) fn resume_native_buffer(
        &mut self,
        key: SessionKey,
        buffer: BufferId,
        shown: bool,
    ) -> NativeOrder {
        // Its bridge asks the person's permission there (ADR 0165, step 2).
        self.start_sessions_socket();
        if let Some(native) = self.hosting.natives.map.get_mut(&key) {
            native.first = false;
            native.ended = None;
            // Its conversation is there: it may be resumed again.
            native.spoke = true;
            native.composing.busy = false;
            native.composing.resumed();
            let change = native.rows.note("Resumed.");
            super::draw(&mut self.editor, native, [change]);
        }
        self.draw_queue(key);
        self.editor.set_running(buffer, true);
        self.set_native_state(key, NativeState::Starting);
        self.hosting.starting.insert(key, buffer);
        if shown {
            self.editor.show(buffer);
            self.composer_go(key, true);
        }
        self.native_order(key, buffer)
    }

    /// A refused start of native session `key`, which shows `buffer`: a resume's buffer, which
    /// the table holds the session in, stays, stopped again, and says so. Whether it was one.
    pub(super) fn native_resume_refused(&mut self, key: SessionKey, buffer: BufferId) -> bool {
        let kept =
            (self.hosting.table.get(key)).is_some_and(|session| session.buffer == Some(buffer));
        let Some(native) = self.hosting.natives.map.get_mut(&key).filter(|_| kept) else {
            return false;
        };
        let change = native.rows.note("It did not resume.");
        super::draw(&mut self.editor, native, [change]);
        self.editor.set_running(buffer, false);
        self.set_native_state(key, NativeState::Stopped);
        self.composer_ended(key);
        true
    }

    /// Starts fork `key`, `name`, natively in a new buffer when `directory` is the root of a
    /// project trusted now, under `mode = "native"`: its order and trust. `None` for a terminal.
    /// It starts with `mode`, the permission mode state of the native session it copies, as
    /// that session's next process would (part 3b).
    pub(in crate::app::sessions::hosting) fn start_native_fork(
        &mut self,
        key: SessionKey,
        name: &str,
        directory: &Path,
        root: Option<&Path>,
        mode: Option<ModeState>,
    ) -> Option<(NativeOrder, TrustedRoot)> {
        if !self.configuration.native_sessions() {
            return None;
        }
        let trusted = self.native_trust_now(directory, root).ok()?;
        let mut order = self.native_session_buffer(key, name, directory.to_path_buf(), true, None);
        if let (Some(mode), Some(native)) = (mode, self.hosting.natives.map.get_mut(&key)) {
            native.mode = mode;
            order.permission_mode = native.mode.for_start();
        }
        Some((order, trusted))
    }
}
