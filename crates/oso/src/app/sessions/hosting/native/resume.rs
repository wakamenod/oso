//! A native session's resume, fork and adoption (ADR 0165, step 2, R1 and the author's Q2 (a);
//! part 4e): a stopped native session resumes natively, in its own buffer, keeping its rows
//! with one "Resumed" row; a fork of a session, or a conversation adopted, at a trusted
//! project's root under `[sessions] mode`'s native is native, in a new buffer. Each starts at
//! once, and the person writes in its composer. `resume_hosted`, `fork_from` and `adopt_into`
//! order them.
//!
//! Must not depend on: key presses or how a frame is drawn; the start thread starts the process.
//!
//! - Only at a project's root trusted now (`trust`): by an open buffer of it, else by the
//!   record of trusted projects, which the start thread judges by the root's identity too
//!   (part 4a), and which looks at it again before the spawn; or, in an issue space's own
//!   directory, under its space's gate (ADR 0176, `spaces::native`); else a resume refuses,
//!   saying why, and a fork runs in a terminal.
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
use crate::sessions::identity::SessionId;
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
    /// which reads its earlier turns if its rows hold none of them, or those after its rows
    /// if its conversation may have gone on elsewhere, the "Resumed" row then drawn after them
    /// (step 5).
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
        let past = id.and_then(|id| self.native_wants_past(key, &id));
        if let Some(native) = self.hosting.natives.map.get_mut(&key) {
            native.first = false;
            native.ended = None;
            // Its conversation is there: it may be resumed again.
            native.spoke = true;
            native.composing.busy = false;
            native.composing.resumed();
        }
        let row = self.resumed_row(key);
        self.native_note_row(key, &row);
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

    /// Starts fork `key`, `name`, of conversation `from`, natively in a new buffer, as
    /// [`App::start_native_new`] does, or in a space's directory under its gate, judged by
    /// `from` (ADR 0176). It starts with `forked`, what the native session it copies holds:
    /// its permission mode state, as that session's next process would (part 3b), and the
    /// model and effort chosen there (part 3c).
    pub(in crate::app::sessions::hosting) fn start_native_fork(
        &mut self,
        key: SessionKey,
        from: &SessionId,
        name: &str,
        directory: &Path,
        root: Option<&Path>,
        forked: Option<Forked>,
    ) -> Option<(NativeOrder, Option<TrustedRoot>)> {
        let gate = match self.configuration.native_sessions() {
            true => self.native_space_gate(key, from, name, directory),
            false => None,
        };
        let (mut order, trusted) = match gate {
            // No gate there: a terminal.
            Some(gate) => {
                let gate = gate.ok()?;
                let order =
                    self.native_session_buffer(key, name, directory.to_path_buf(), true, None);
                (NativeOrder { gate, ..order }, None)
            }
            None => self.start_native_new(key, name, directory, root, true)?,
        };
        if let (Some(forked), Some(native)) = (forked, self.hosting.natives.map.get_mut(&key)) {
            native.mode = forked.mode;
            order.permission_mode = native.mode.for_start();
            native.choice = forked.choice;
            (order.model, order.effort) = native.choice.for_start();
        }
        Some((order, trusted))
    }

    /// Starts adopted conversation `key`, `id`, `name`, natively in a new buffer, as
    /// [`App::start_native_new`] does (part 4e). It has a conversation, which it may resume
    /// again; its earlier turns, which a resume writes none of and the TUI showed, are read
    /// from its transcript (part 5a), and one "Adopted" row follows them.
    pub(in crate::app::sessions::hosting) fn start_native_adopt(
        &mut self,
        key: SessionKey,
        id: &SessionId,
        name: &str,
        directory: &Path,
        root: &Path,
        shown: bool,
    ) -> Option<(NativeOrder, Option<TrustedRoot>)> {
        let (order, trusted) = self.start_native_new(key, name, directory, Some(root), shown)?;
        if let Some(native) = self.hosting.natives.map.get_mut(&key) {
            native.spoke = true;
        }
        let past = self.native_wants_past(key, id);
        self.native_note_row(key, "Adopted.");
        Some((NativeOrder { past, ..order }, trusted))
    }

    /// Starts `key`, `name`, natively in a new buffer, shown if `shown`, when `directory` is
    /// the root of a project trusted now, by an open buffer of it or the record (part 4a),
    /// unless `[sessions] mode = "tui"`: its order, and the trust an open buffer gave. `None`
    /// for a terminal: a fork's or an adoption's.
    fn start_native_new(
        &mut self,
        key: SessionKey,
        name: &str,
        directory: &Path,
        root: Option<&Path>,
        shown: bool,
    ) -> Option<(NativeOrder, Option<TrustedRoot>)> {
        if !self.configuration.native_sessions() {
            return None;
        }
        // A record not read yet: `native_waits_for_trust` held the start until it was, or the
        // wait ran out, so this runs in a terminal.
        let (trusted, trust) = match self.native_trust_listed(directory, root).ok()? {
            Judged::Waits => return None,
            judged => self.native_judged(key, judged),
        };
        let order = self.native_session_buffer(key, name, directory.to_path_buf(), shown, None);
        Some((NativeOrder { trust, ..order }, trusted))
    }

    /// Whether a fork or an adoption in `directory`, of the project at `root`, waits for the
    /// record of trusted projects before it is judged native or not (parts 4a and 4e), as a
    /// fork waits whole (ADR 0173).
    pub(in crate::app::sessions::hosting) fn native_waits_for_trust(
        &self,
        directory: &Path,
        root: Option<&Path>,
    ) -> bool {
        // Judged only while the record is unread, which copies nothing of it; a space's
        // directory is judged by its space (ADR 0176).
        self.configuration.native_sessions()
            && matches!(self.trust_record(), TrustRecord::Unread)
            && !self.is_space_directory(directory)
            && matches!(self.native_trust_listed(directory, root), Ok(Judged::Waits))
    }
}
