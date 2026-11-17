//! A session started outside oso moving into a space, once the person agreed (ADR 0151): a
//! thread watches the outside process, and once that has ended the conversation is resumed as
//! `session-adopt` resumes one, in that space.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here touches the disk or
//! the process table: the wait's thread does, and the start thread checks the registry again
//! before the resume, so that two processes never run one conversation.
//!
//! - One move waits at a time. Killing its `*moving: NAME*` buffer cancels it.
//! - A move that gives up, or an oso exit while it waits, resumes nothing.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use oso_core::buffer::{BufferId, Mode};

use super::adopt::Landing;
use crate::app::{App, UserEvent};
use crate::git::printable;
use crate::sessions::adopt::Pick;
use crate::sessions::moving::{self, Asker, Step};
use crate::sessions::spaces::SpaceId;
use crate::sessions::tools::quoted;
use crate::sessions::worker::SessionKey;

/// What a move does on the person's `y`.
#[derive(Debug)]
pub(in crate::app) struct Moving {
    pub(in crate::app) asker: Asker,
    /// What the person is told it is called.
    pub(in crate::app) name: String,
    pub(in crate::app) space: SpaceId,
    pub(in crate::app) space_name: String,
    /// Its repository joins the space.
    pub(in crate::app) adds: bool,
    /// The stopped session of oso's record with its id, which resumes in its own buffer.
    pub(in crate::app) reuse: Option<SessionKey>,
}

/// A move that waits for the outside process to end.
#[derive(Debug)]
struct Waiting {
    token: u64,
    moving: Moving,
    /// `*moving: NAME*`, whose kill cancels it.
    buffer: BufferId,
    /// Ends the wait's thread.
    stop: Arc<AtomicBool>,
}

/// What the app keeps of moves.
#[derive(Debug, Default)]
pub(in crate::app) struct Moves {
    next: u64,
    waiting: Option<Waiting>,
    /// The space each moved session being started goes in, by its key.
    pub(in crate::app) landing: HashMap<SessionKey, SpaceId>,
}

impl Moves {
    /// Whether a move waits for its outside process.
    pub(in crate::app) fn waits(&self) -> bool {
        self.waiting.is_some()
    }

    /// The next token, for a look or a wait.
    pub(in crate::app) fn token(&mut self) -> u64 {
        self.next += 1;
        self.next
    }
}

impl App {
    /// Watches `moving`'s outside process on a thread of its own, with a buffer that says so.
    pub(in crate::app) fn wait_for_move(&mut self, moving: Moving) {
        let token = self.hosting.moves.token();
        let said = moving::waiting(&moving.name, &moving.space_name);
        let buffer = self.editor.add_internal_buffer(
            format!("*moving: {}*", printable(&moving.name, 40)),
            Mode::Fundamental,
        );
        self.editor.set_internal_text(buffer, &format!("{said}\n"));
        let stop = Arc::new(AtomicBool::new(false));
        let waker = Mutex::new(self.waker.clone());
        let spawned = moving::spawn_wait(
            moving.asker.pid,
            moving.asker.started,
            Arc::clone(&stop),
            move |step| {
                (waker.lock().unwrap_or_else(PoisonError::into_inner))
                    .send(UserEvent::MoveWaited(token, step))
            },
        );
        if let Err(e) = spawned {
            log::warn!("cannot watch a session that moves: {e}");
            self.editor.kill_buffer(buffer);
            return self.editor.message(moving::gave_up(&moving.name));
        }
        self.editor.message(said);
        self.hosting.moves.waiting = Some(Waiting {
            token,
            moving,
            buffer,
            stop,
        });
        self.request_redraw();
    }

    /// The wait `token` took `step`.
    pub(in crate::app) fn move_waited(&mut self, token: u64, step: Step) {
        let moves = &mut self.hosting.moves;
        if moves.waiting.as_ref().is_none_or(|w| w.token != token) {
            return;
        }
        match step {
            Step::Wait => {}
            Step::Late => {
                if let Some(waiting) = &moves.waiting {
                    let said = moving::late(&waiting.moving.name);
                    self.editor.message(said);
                }
            }
            Step::GiveUp => {
                if let Some(waiting) = self.end_wait() {
                    self.editor.message(moving::gave_up(&waiting.moving.name));
                }
            }
            Step::Stopped => {
                if let Some(waiting) = self.end_wait() {
                    self.resume_moved(waiting.moving);
                }
            }
        }
        self.request_redraw();
    }

    /// The waiting move, its buffer gone and its thread told to end.
    fn end_wait(&mut self) -> Option<Waiting> {
        let waiting = self.hosting.moves.waiting.take()?;
        waiting.stop.store(true, Ordering::Release);
        if self.editor.buffer(waiting.buffer).is_some() {
            self.editor.kill_buffer(waiting.buffer);
        }
        Some(waiting)
    }

    /// After each event: a move whose buffer the person killed is cancelled.
    pub(in crate::app) fn sync_moves(&mut self) {
        let Some(waiting) = &self.hosting.moves.waiting else {
            return;
        };
        if self.editor.buffer(waiting.buffer).is_some() {
            return;
        }
        if let Some(waiting) = self.end_wait() {
            self.editor.message(format!(
                "The move of {} was cancelled; oso resumes nothing",
                quoted(&waiting.moving.name)
            ));
            self.request_redraw();
        }
    }

    /// The outside process of `moving` ended: its conversation resumes, in its space.
    fn resume_moved(&mut self, moving: Moving) {
        let spaces = self.watch.spaces();
        let at = spaces.find(&moving.space);
        let shown = at.is_some() && spaces.current() == at;
        if let Some(key) = moving.reuse {
            return self.resume_moved_record(key, &moving);
        }
        let pick = Pick {
            id: moving.asker.id.clone(),
            directory: Some(moving.asker.directory.clone()),
            name: moving.asker.name.clone(),
            running: None,
            busy: false,
        };
        self.adopt_into(
            pick,
            Some(Landing {
                space: moving.space,
                space_name: moving.space_name,
                shown,
            }),
        );
    }

    /// `key`, the stopped session of oso's record that `moving` moves, resumes in its own
    /// buffer, in `moving`'s space now (ADR 0151, Q5).
    fn resume_moved_record(&mut self, key: SessionKey, moving: &Moving) {
        let Some(session) = self.hosting.table.get_mut(key).filter(|s| !s.runs()) else {
            return self.editor.message(format!(
                "Session {} runs in oso already",
                quoted(&moving.name)
            ));
        };
        session.space = Some(moving.space.clone());
        let buffer = session.buffer;
        self.hosting.unsaved = true;
        self.sessions_touched();
        match buffer.filter(|&buffer| self.editor.buffer(buffer).is_some()) {
            Some(buffer) => {
                self.resume_session(buffer);
                self.editor.message(format!(
                    "Moved {} into space {}",
                    quoted(&moving.name),
                    quoted(&moving.space_name)
                ));
            }
            None => self.editor.message(format!(
                "Session {} is in space {} now; resume it from *Spaces*",
                quoted(&moving.name),
                quoted(&moving.space_name)
            )),
        }
    }
}
