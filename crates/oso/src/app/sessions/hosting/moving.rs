//! A session started outside oso moving into a space, once the person agreed (ADR 0151): a
//! thread watches the outside process, and once that has ended the conversation is resumed as
//! `session-adopt` resumes one, in that space.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here touches the disk or
//! the process table: the wait's thread does, and the start thread checks the registry again
//! before the resume, so that two processes never run one conversation.
//!
//! - One move waits at a time. Killing its `*moving: NAME*` buffer cancels it.
//! - The bridge of the session that moves holds a `move_wait` call (step 2). Once the wait's
//!   thread sees in the registry that the session's turn ended, the reply names the process,
//!   and the bridge stops its own parent if that is the one; oso itself signals nothing. At
//!   120 s, or when the move ends any other way, the reply says why not, and the person's
//!   `/exit` is what is left.
//! - A move that gives up, an oso exit while it waits, or a space closed or left by the
//!   repository meanwhile resumes nothing.
//! - What the echo area says names the session as the registry does, so none of it is logged.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, PoisonError};

use serde_json::json;

use oso_core::buffer::{BufferId, Mode};

use super::adopt::Landing;
use crate::app::{App, UserEvent};
use crate::channel::{Connection, Reply};
use crate::git::printable;
use crate::sessions::adopt::Pick;
use crate::sessions::identity::Origin;
use crate::sessions::moving::{self, Asker, BridgeWait, Step, TOO_LATE, Watched};
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
    /// The connection of the session's bridge, whose `move_wait` alone is held.
    connection: Connection,
    /// The bridge's `move_wait`, asked once and answered once.
    bridge: BridgeWait<Sender<Reply>>,
}

/// What the app keeps of moves.
#[derive(Debug, Default)]
pub(in crate::app) struct Moves {
    next: u64,
    waiting: Option<Waiting>,
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
    /// `connection` is its bridge's, which the agent was told at `since_ms` to end its turn.
    pub(in crate::app) fn wait_for_move(
        &mut self,
        moving: Moving,
        connection: Connection,
        since_ms: u64,
    ) {
        let token = self.hosting.moves.token();
        let said = moving::waiting(&moving.name, &moving.space_name);
        let buffer = self.editor.add_internal_buffer(
            format!("*moving: {}*", printable(&moving.name, 40)),
            Mode::SessionMove,
        );
        self.editor.set_internal_text(buffer, &format!("{said}\n"));
        let stop = Arc::new(AtomicBool::new(false));
        let waker = Mutex::new(self.waker.clone());
        let watched = Watched {
            pid: moving.asker.pid,
            then: moving.asker.started,
            id: moving.asker.id.clone(),
            since_ms,
        };
        let spawned = moving::spawn_wait(watched, Arc::clone(&stop), move |step| {
            (waker.lock().unwrap_or_else(PoisonError::into_inner))
                .send(UserEvent::MoveWaited(token, step))
        });
        if let Err(e) = spawned {
            log::warn!("cannot watch a session that moves: {e}");
            self.editor.kill_buffer(buffer);
            return self.editor.message_unlogged(moving::gave_up(&moving.name));
        }
        self.editor.message_unlogged(said);
        self.hosting.moves.waiting = Some(Waiting {
            token,
            moving,
            buffer,
            stop,
            connection,
            bridge: BridgeWait::default(),
        });
        self.request_redraw();
    }

    /// `move_wait` from `connection`: held until the moving session's turn ended, then answered
    /// with the process its bridge stops (ADR 0151, step 2). Only the bridge of the session
    /// that moves is held, once.
    pub(in crate::app) fn move_wait(&mut self, connection: Connection, reply: Sender<Reply>) {
        let waiting = (self.hosting.moves.waiting.as_mut())
            .filter(|waiting| waiting.connection == connection);
        let Some(waiting) = waiting else {
            let _ = reply.send(Err(moving::NOT_WAITING.into()));
            return;
        };
        if let Err((reply, why)) = waiting.bridge.ask(reply) {
            let _ = reply.send(Err(why.into()));
        }
    }

    /// The wait `token` took `step`.
    pub(in crate::app) fn move_waited(&mut self, token: u64, step: Step) {
        let moves = &mut self.hosting.moves;
        if moves.waiting.as_ref().is_none_or(|w| w.token != token) {
            return;
        }
        match step {
            Step::Wait => {}
            Step::Idle => {
                let held = moves.waiting.as_mut().and_then(|w| w.bridge.turn_ended());
                if let (Some(held), Some(waiting)) = (held, &moves.waiting) {
                    let _ = held.send(Ok(json!({ "stop": waiting.moving.asker.pid })));
                    self.agent_log("move_to_space: the turn ended; its bridge stops the session");
                }
            }
            Step::Late => {
                if let Some(waiting) = &mut moves.waiting {
                    if let Some(held) = waiting.bridge.late() {
                        let _ = held.send(Err(TOO_LATE.into()));
                    }
                    let said = moving::late(&waiting.moving.name);
                    self.editor.message_unlogged(said);
                }
            }
            Step::GiveUp => {
                if let Some(waiting) = self.end_wait() {
                    self.editor
                        .message_unlogged(moving::gave_up(&waiting.moving.name));
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
        let mut waiting = self.hosting.moves.waiting.take()?;
        waiting.stop.store(true, Ordering::Release);
        if let Some(held) = waiting.bridge.ended() {
            let _ = held.send(Err("the move ended".into()));
        }
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
            self.editor.message_unlogged(format!(
                "The move of {} was cancelled; oso resumes nothing",
                quoted(&waiting.moving.name)
            ));
            self.request_redraw();
        }
    }

    /// The outside process of `moving` ended: its conversation resumes, in its space, if
    /// oso still trusts its repository and that space is still open and holds it.
    fn resume_moved(&mut self, moving: Moving) {
        if self.member_trust(&moving.asker.main).is_none() {
            let why = moving::untrusted_meanwhile(&moving.name);
            return self.editor.message_unlogged(why);
        }
        let spaces = self.watch.spaces();
        let Some(at) = spaces.find(&moving.space).filter(|&at| {
            spaces.get(at).is_some_and(|space| space.is_open())
                && (spaces.stored(at)).is_some_and(|s| s.has_member(&moving.asker.main.root))
        }) else {
            return (self.editor)
                .message_unlogged(moving::space_gone(&moving.name, &moving.space_name));
        };
        // Behind while a native session's request waits where it would be shown (ADR 0182).
        let shown = spaces.current() == Some(at) && !self.request_waits().holds_shown();
        if let Some(key) = moving.reuse {
            return self.resume_moved_record(key, &moving, at, shown);
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
    /// buffer, in `moving`'s space now, the space at `at`, shown if it is in front, and as
    /// started on the person's `y` (ADR 0151, Q4 and Q5).
    fn resume_moved_record(&mut self, key: SessionKey, moving: &Moving, at: usize, shown: bool) {
        let Some(session) = self.hosting.table.get_mut(key).filter(|s| !s.runs()) else {
            return self.editor.message_unlogged(format!(
                "Session {} runs in oso already",
                quoted(&moving.name)
            ));
        };
        session.space = Some(moving.space.clone());
        session.origin = Origin::Answered;
        let buffer = session.buffer;
        self.hosting.unsaved = true;
        self.sessions_touched();
        match buffer.filter(|&buffer| self.editor.buffer(buffer).is_some()) {
            Some(buffer) => {
                if !shown {
                    self.watch.spaces_mut().showed(at, Some(buffer), None);
                }
                // Judged where it runs, but never by a worktree's claim.
                self.resume_session_shown(buffer, shown, false);
                self.editor.message_unlogged(format!(
                    "Moved {} into space {}",
                    quoted(&moving.name),
                    quoted(&moving.space_name)
                ));
            }
            None => self.editor.message_unlogged(format!(
                "Session {} is in space {} now; resume it from *Spaces*",
                quoted(&moving.name),
                quoted(&moving.space_name)
            )),
        }
    }
}
