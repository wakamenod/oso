//! `move_to_space` in the app (ADR 0151): a session started outside oso asks to move into an
//! open space. A thread finds who asks from Claude Code's registry, by the bridge's parent;
//! the checks and the question follow, as for the other tools that ask; on the person's `y`
//! the space takes the repository, the agent is told to end its turn, and `hosting::moving`
//! waits for the outside process to end before it resumes the conversation.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here touches the disk.
//!
//! - Only an outside connection the person let in may call it, whose process runs oso's own
//!   executable, as `oso mcp` does.
//! - The agent names only the space; the conversation, its directory and its process are the
//!   registry entry's of the bridge's parent, as it was when the connection came.
//! - The person's refusals are kept by the conversation's id.

use std::sync::{Mutex, PoisonError};
use std::time::Instant;

use serde_json::json;

use super::super::super::{App, UserEvent};
use super::super::hosting::Moving;
use super::asks::Plan;
use super::{Pending, QUESTION_WAITS};
use crate::sessions::moving::{self, Asker, Look};
use crate::sessions::spaces::SpaceId;
use crate::sessions::spaces::edit::NotAdded;
use crate::sessions::spaces::stored::{MAX_MEMBERS, now_ms};
use crate::sessions::tools::quoted;

/// The tool's name, as its table and its refusals have it.
const TOOL: &str = "move_to_space";

/// A call whose asker is being looked for.
#[derive(Debug)]
pub(in crate::app) struct Looking {
    pending: Pending,
    token: u64,
    space: String,
}

impl Looking {
    pub(super) fn connection(&self) -> crate::channel::Connection {
        self.pending.connection
    }
}

impl App {
    /// `move_to_space` from the outside connection of `pending`, into the space named `space`.
    pub(super) fn tool_move_to_space(&mut self, pending: Pending, space: String) {
        if let Err(why) = self.may_look_for_a_move(&pending, &space) {
            self.agent_log(&format!("{TOOL}: refused"));
            return pending.answer(Err(why));
        }
        let Some(outsider) = self.outsider(pending.connection) else {
            return pending.answer(Err("oso does not know this connection's process".into()));
        };
        let (pid, program, started) = match (&outsider.peer.parent, outsider.parent_started) {
            (Some(parent), Some(started)) => {
                (parent.pid(), parent.name().map(str::to_owned), started)
            }
            _ => {
                return pending.answer(Err(
                    "the system does not say which process started this connection, or when".into(),
                ));
            }
        };
        let look = Look {
            pid,
            started,
            program,
            trusted: self.trusted_list(),
            home: oso_platform::privacy::home(),
        };
        let token = self.hosting.moves.token();
        let waker = Mutex::new(self.waker.clone());
        let spawned = moving::spawn_look(look, move |found| {
            (waker.lock().unwrap_or_else(PoisonError::into_inner))
                .send(UserEvent::MoveLooked(token, Box::new(found)));
        });
        if let Err(e) = spawned {
            log::warn!("cannot look for a session that asks to move: {e}");
            return pending.answer(Err("oso cannot look for this session now".into()));
        }
        self.hosting.tools.looking = Some(Looking {
            pending,
            token,
            space,
        });
    }

    /// Why the call of `pending` into `space` is refused before anything is looked at.
    fn may_look_for_a_move(&self, pending: &Pending, space: &str) -> Result<(), String> {
        let outsider = self
            .outsider(pending.connection)
            .ok_or("oso does not know this connection's process")?;
        // Only `oso mcp`, which Claude Code starts as its child, is known by its parent.
        if !outsider.peer.runs_this_program {
            return Err("only oso mcp, started by a Claude Code session, may ask to move".into());
        }
        self.open_space(space)?;
        if self.hosting.tools.looking.is_some() || self.hosting.moves.waits() {
            return Err("another session's move waits; try again once it is done".into());
        }
        if self.agent_question_waits() {
            return Err(QUESTION_WAITS.into());
        }
        Ok(())
    }

    /// The open space named `name`, its slug, by its place and its id: an agent names a space
    /// by its slug alone, which never changes (ADR 0159).
    fn open_space(&self, name: &str) -> Result<(usize, SpaceId), String> {
        let spaces = self.watch.spaces();
        let found = spaces
            .with_slug(name)
            .and_then(|at| Some((at, spaces.get(at)?)));
        match found {
            Some((at, space)) if space.is_open() => Ok((at, space.id().clone())),
            Some(_) => Err(format!(
                "space {} is closed: the person reopens it with space-reopen",
                quoted(name)
            )),
            None => Err(format!("no open space is named {}", quoted(name))),
        }
    }

    /// The look `token` found who asks, or why it cannot move.
    pub(in crate::app) fn move_looked(&mut self, token: u64, found: Result<Asker, String>) {
        let tools = &mut self.hosting.tools;
        let Some(looking) = tools.looking.take_if(|looking| looking.token == token) else {
            return;
        };
        let Looking { pending, space, .. } = looking;
        let asked = found.and_then(|asker| self.move_question(&space, asker));
        match asked {
            Ok((caller, label, details, plan)) => {
                self.queue_question(pending, (caller, space), (label, details), plan);
            }
            Err(why) => {
                self.agent_log(&format!("{TOOL}: refused"));
                pending.answer(Err(why));
            }
        }
    }

    /// The question of `asker`'s move into the space named `space`, what `?` shows, and what
    /// the person's `y` does; or why it is refused.
    fn move_question(
        &mut self,
        space: &str,
        asker: Asker,
    ) -> Result<(crate::sessions::identity::SessionId, String, String, Plan), String> {
        let (at, id) = self.open_space(space)?;
        let name = moving::called(asker.name.as_deref(), &asker.id);
        // A conversation oso runs would fork; a stopped one of its record resumes in its own
        // buffer, where it ran.
        let spaces = self.watch.spaces();
        let found = self.hosting.table().iter().find(|s| s.id == asker.id);
        let record = found.map(|session| moving::Recorded {
            name: &session.name,
            runs: session.runs(),
            directory: &session.directory,
            orchestrates: (spaces.iter())
                .any(|s| s.stored.orchestrator.as_ref() == Some(&session.id)),
        });
        let stored = spaces.stored(at).ok_or("the space is gone")?;
        let held = moving::Held {
            record,
            trusted: self.member_trust(&asker.main).is_some(),
            member: stored.has_member(&asker.main.root),
            members: stored.members.len(),
        };
        let adds = moving::judge(&asker, space, &held)?;
        let reuse = found.map(|session| session.key);
        // The space that holds the worktree now, which the move takes it from.
        let held_by = (asker.worktree.as_deref())
            .and_then(|worktree| spaces.holding(worktree))
            .filter(|&other| other != at)
            .and_then(|other| spaces.get(other))
            .map(|other| other.label().to_owned());
        // Before the guard, so that a call refused here does not count toward the rate
        // (ADR 0178).
        if self.agent_question_waits() {
            return Err(QUESTION_WAITS.into());
        }
        (self.hosting.tools.guard)
            .admit(&asker.id, TOOL, space, Instant::now())
            .map_err(|refusal| refusal.why())?;
        let added = adds.then_some(asker.main.root.as_path());
        // The question quotes the slug the agent gave whole; `?` says what the person calls
        // the space, cleaned, in full (ADR 0159).
        let display = stored.display.clone();
        let label = moving::question(&name, &asker, space, added);
        let mut details = moving::details(&label, &asker, space, held_by.as_deref());
        if !display.is_empty() {
            details.push_str(&format!(
                "\n\nSpace {} is the one you call {}.",
                quoted(space),
                crate::app::sessions::spaces::printable(&display)
            ));
        }
        let caller = asker.id.clone();
        let plan = Plan::Move(Box::new(Moving {
            asker,
            name,
            space: id,
            space_name: space.to_owned(),
            adds,
            reuse,
        }));
        Ok((caller, label, details, plan))
    }

    /// The person's `y` to `moving`: the space takes its repository, the agent is told to end
    /// its turn, and the wait for its process begins.
    pub(super) fn start_move(&mut self, pending: Pending, moving: Moving) {
        let spaces = self.watch.spaces();
        let Some(at) = spaces
            .find(&moving.space)
            .filter(|&at| spaces.get(at).is_some_and(|space| space.is_open()))
        else {
            return pending.answer(Err("the space closed meanwhile".into()));
        };
        if self.member_trust(&moving.asker.main).is_none() {
            return pending.answer(Err("the repository is no longer trusted".into()));
        }
        let now = now_ms();
        if moving.adds {
            let member = moving.asker.main.clone();
            match self.watch.spaces_mut().add_member(at, member, now) {
                Ok(()) | Err(NotAdded::Already) => {}
                Err(NotAdded::Full) => {
                    return pending.answer(Err(format!(
                        "the space holds {MAX_MEMBERS} repositories, the most it may"
                    )));
                }
            }
        }
        if let Some(worktree) = &moving.asker.worktree {
            self.watch.spaces_mut().hold_worktree(at, worktree, now);
        }
        self.sessions_touched();
        let connection = pending.connection;
        pending.answer(Ok(json!({
            "moving": true,
            "space": moving.space_name,
            "next": moving::REPLY,
        })));
        self.agent_log(&format!("{TOOL}: allowed; waiting for the session to stop"));
        self.wait_for_move(moving, connection, now);
    }
}
