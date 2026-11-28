//! A stopped native session's earlier turns (ADR 0165, part 5c, the author's ask of
//! 2026-11-28): a session brought back at oso's start with no process reads them from Claude
//! Code's transcript once a window first shows its buffer, so that it shows its conversation
//! with no resume; here the read is started and what it gave drawn as step 5 draws a start's.
//!
//! Must not depend on: key presses or how a frame is drawn; the read runs on a thread of its
//! own, `oso-native-past`, and nothing waits for it.
//!
//! - Only once its buffer is shown, not at oso's start for each one listed: a transcript is
//!   read, and its rows kept, only for a buffer the person looks at. Each is read at most once
//!   an instance; one that did not read says why and asks again at its resume.
//! - Step 5's limits, notes and rules of trust apply as they are: it is the same read of the
//!   same file a resume would make.
//! - The rows then hold the conversation, so a resume asks only for what follows them (part
//!   5b). A start of any kind ends the read's turn: what it gives after is dropped, as the
//!   start reads for itself, and its rows come before anything its process says.
//! - A session archived, or forgotten, meanwhile has no rows to draw it in: it is dropped.

use crate::app::{App, UserEvent};
use crate::sessions::hosted::Run;
use crate::sessions::native::past::{Missing, Past, PastAsk};
use crate::sessions::past::read_for_stopped;
use crate::sessions::worker::SessionKey;

/// What the rows of a session brought back stopped say last: how to resume it.
pub(super) const HOW_TO_RESUME: &str =
    "Stopped: C-c a r resumes it here, or r on its row in *Spaces*.";

/// Where a stopped session's read of its earlier turns is (part 5c).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum StoppedPast {
    /// None is to be read: it was not brought back stopped.
    #[default]
    None,
    /// Brought back stopped: read once its buffer is first shown.
    Unread,
    /// Its read runs.
    Reading,
    /// Read, or a start read for itself.
    Over,
}

impl App {
    /// Native session `key`, brought back stopped, reads its earlier turns once its buffer is
    /// first shown.
    pub(super) fn native_stopped_unread(&mut self, key: SessionKey) {
        if let Some(native) = self.hosting.natives.map.get_mut(&key) {
            native.stopped_past = StoppedPast::Unread;
        }
    }

    /// After each event: the read of each stopped session's earlier turns whose buffer a
    /// window shows now for the first time starts. With none to read, no window is looked at.
    pub(in crate::app) fn stopped_pasts_due(&mut self) {
        let natives = &self.hosting.natives.map;
        if !(natives.values()).any(|native| native.stopped_past == StoppedPast::Unread) {
            return;
        }
        let shown: Vec<SessionKey> = (natives.iter())
            .filter(|(_, native)| native.stopped_past == StoppedPast::Unread)
            .filter(|(_, native)| {
                (self.editor.windows()).any(|window| window.buffer() == native.buffer)
            })
            .map(|(key, _)| *key)
            .collect();
        for key in shown {
            self.read_stopped_past(key);
        }
    }

    /// Starts the read of stopped session `key`'s earlier turns.
    fn read_stopped_past(&mut self, key: SessionKey) {
        let Some(session) = self.hosting.table.get(key) else {
            return;
        };
        let ask = PastAsk {
            id: session.id.clone(),
            after: Vec::new(),
        };
        let directory = session.directory.clone();
        // Whose brief its first message is, as a start's read says (part 5a).
        let from = self.hosting.table.started_by(&ask.id);
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        native.stopped_past = StoppedPast::Reading;
        native.rows.note_started_by(from);
        let waker = self.waker.clone();
        let started = read_for_stopped(ask, directory, move |past| {
            waker.send(UserEvent::StoppedPast(key, Box::new(past)));
        });
        if started.is_err() {
            self.native_stopped_past(key, Past::missing(Missing::NotRead));
        }
    }

    /// The read of stopped session `key`'s earlier turns gave `past`: drawn, as a start's
    /// are, then how to resume it, unless it has started since, or gone.
    pub(in crate::app) fn native_stopped_past(&mut self, key: SessionKey, past: Past) {
        let stopped = (self.hosting.table.get(key)).is_some_and(|session| {
            !session.archived && matches!(session.run, Run::Stopped | Run::Failed)
        });
        let starting = self.hosting.starting.contains_key(&key);
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        if native.stopped_past != StoppedPast::Reading {
            return;
        }
        native.stopped_past = StoppedPast::Over;
        if !stopped || starting || native.starting() || native.stream.is_some() {
            return;
        }
        let read = past.missing.is_none();
        native.notes_after_past = Some(vec![HOW_TO_RESUME.to_owned()]);
        self.native_past(key, past);
        if let Some(native) = self.hosting.natives.map.get_mut(&key) {
            // Stopped, it may be resumed elsewhere before here: its resume reads what follows.
            native.may_have_gone_on = read;
        }
    }

    /// Native session `key` starts, or is handed to the TUI: a read of its earlier turns as a stopped one is over, and
    /// what it gives dropped, as the start reads for itself.
    pub(super) fn native_stopped_past_over(&mut self, key: SessionKey) {
        if let Some(native) = self.hosting.natives.map.get_mut(&key)
            && native.stopped_past != StoppedPast::None
        {
            native.stopped_past = StoppedPast::Over;
        }
    }
}
