//! A native session's resume and fork (ADR 0165, step 2, R1 and the author's Q2 (a)): a
//! stopped native session resumes natively, in its own buffer, keeping its rows with one
//! "Resumed" row; a fork of a session at a trusted project's root under `mode = "native"` is
//! native, in a new buffer. Each starts at once, and the person writes in its composer. `resume_hosted` and `fork_from` order them.
//!
//! Must not depend on: key presses or how a frame is drawn; the start thread starts the process.
//!
//! - Only at a project's root trusted now (`trust`): by an open buffer of it, else by the
//!   record of trusted projects, which the start thread judges by the root's identity too
//!   (part 4a), and which looks at it again before the spawn; else a resume refuses, saying
//!   why, and a fork runs in a terminal.
//! - A resume takes no baseline: the session keeps the one it had.
//! - A resume and a fork start in the mode the session was in (part 3b), as `--resume` keeps
//!   none, and with the model and effort the person chose (part 3c): a fork, its source's.
//! - `claude -p --resume` writes no earlier turn on its stream (a probe of 2026-11-22): the
//!   rows kept are the transcript, and rows that hold none of it, as in a new instance, are
//!   given its earlier turns from Claude Code's transcript (step 5, `past`).

use std::path::Path;

use oso_core::buffer::BufferId;

use super::note::NativeState;
use crate::app::App;
use crate::git::TrustedRoot;
use crate::sessions::native::choice::Choice;
use crate::sessions::native::mode::ModeState;
use crate::sessions::start::NativeOrder;
use crate::sessions::start::trust::{Judged, NotTrusted, TrustRecord};
use crate::sessions::worker::SessionKey;

/// What a native fork starts with of the session it copies (parts 3b and 3c).
pub(in crate::app::sessions::hosting) struct Forked {
    pub(in crate::app::sessions::hosting) mode: ModeState,
    pub(in crate::app::sessions::hosting) choice: Choice,
}

impl Forked {
    /// What a fork of `native` starts with.
    pub(super) fn of(native: &super::Native) -> Self {
        Self {
            mode: native.mode.forked(),
            choice: native.choice.forked(),
        }
    }
}

impl App {
    /// Where the trust native session `name`, in `directory` of the project at `root`,
    /// resumes under comes from: an open buffer of its project, else the record of trusted
    /// projects, read or waited for (part 4a).
    ///
    /// # Errors
    ///
    /// Why it does not resume natively, and what does.
    pub(in crate::app::sessions::hosting) fn native_resume_trust(
        &self,
        name: &str,
        directory: &Path,
        root: Option<&Path>,
    ) -> Result<Judged, String> {
        let fork = "session-fork continues it in a terminal";
        self.native_trust_listed(directory, root)
            .map_err(|why| match why {
                NotTrusted::NotOpen => format!(
                    "Session {name}'s project is not open, and the record of trusted projects \
                     does not hold it: open a file of it to resume it natively, or {fork}"
                ),
                NotTrusted::Untrusted => {
                    format!("Session {name}'s project is not trusted now: {fork}")
                }
                NotTrusted::NotAtRoot => {
                    format!("Session {name} runs below its project's root: {fork}")
                }
                NotTrusted::NoRecord => format!(
                    "Session {name}'s project's trust is not known, as the record of trusted \
                     projects is not read: {fork}"
                ),
            })
    }

    /// Native session `key`'s `buffer` made ready to run again: a "Resumed" row, no turn under
    /// way, as no message is written yet, and a queue that ended waiting for an empty
    /// `C-c C-c`; shown, with the composer selected, if `shown`. The start thread's order,
    /// which reads its earlier turns if its rows hold none of them, the "Resumed" row then
    /// drawn after them (step 5).
    pub(in crate::app::sessions::hosting) fn resume_native_buffer(
        &mut self,
        key: SessionKey,
        buffer: BufferId,
        shown: bool,
    ) -> NativeOrder {
        // Its bridge asks the person's permission there (ADR 0165, step 2).
        self.start_sessions_socket();
        let id = self
            .hosting
            .table
            .get(key)
            .map(|session| session.id.clone());
        let past = id.filter(|_| self.native_wants_past(key));
        if let Some(native) = self.hosting.natives.map.get_mut(&key) {
            native.first = false;
            native.ended = None;
            // Its conversation is there: it may be resumed again.
            native.spoke = true;
            native.composing.busy = false;
            native.composing.resumed();
        }
        self.native_note_row(key, "Resumed.");
        self.draw_queue(key);
        // Its to-do list is not read back: empty until its next to-do call (part 3e).
        self.clear_todo(key);
        self.editor.set_running(buffer, true);
        self.set_native_state(key, NativeState::Starting);
        self.hosting.starting.insert(key, buffer);
        if shown {
            self.editor.show(buffer);
            self.composer_go(key, true);
        }
        NativeOrder {
            past,
            ..self.native_order(key, buffer)
        }
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
    /// project trusted now, by an open buffer of it or the record (part 4a), under
    /// `mode = "native"`: its order and the trust an open buffer gave. `None` for a terminal.
    /// It starts with `forked`, what the native session it copies holds: its permission mode
    /// state, as that session's next process would (part 3b), and the model and effort chosen
    /// there (part 3c).
    pub(in crate::app::sessions::hosting) fn start_native_fork(
        &mut self,
        key: SessionKey,
        name: &str,
        directory: &Path,
        root: Option<&Path>,
        forked: Option<Forked>,
    ) -> Option<(NativeOrder, Option<TrustedRoot>)> {
        if !self.configuration.native_sessions() {
            return None;
        }
        // A record not read yet: `fork_waits_for_trust` held the fork until it was, or the
        // wait ran out, so this runs in a terminal.
        let (trusted, trust) = match self.native_trust_listed(directory, root).ok()? {
            Judged::Waits => return None,
            judged => self.native_judged(key, judged),
        };
        let mut order = self.native_session_buffer(key, name, directory.to_path_buf(), true, None);
        if let (Some(forked), Some(native)) = (forked, self.hosting.natives.map.get_mut(&key)) {
            native.mode = forked.mode;
            order.permission_mode = native.mode.for_start();
            native.choice = forked.choice;
            (order.model, order.effort) = native.choice.for_start();
        }
        Some((NativeOrder { trust, ..order }, trusted))
    }

    /// Whether a fork in `directory`, of the project at `root`, waits for the record of trusted
    /// projects before it is judged native or not (part 4a), as a fork waits whole (ADR 0173).
    pub(in crate::app::sessions::hosting) fn fork_waits_for_trust(
        &self,
        directory: &Path,
        root: Option<&Path>,
    ) -> bool {
        // Judged only while the record is unread, which copies nothing of it.
        self.configuration.native_sessions()
            && matches!(self.trust_record(), TrustRecord::Unread)
            && matches!(self.native_trust_listed(directory, root), Ok(Judged::Waits))
    }
}
