//! `session_archive` (ADR 0179): an agent archives a session's row, as the person's `A` in
//! `*Spaces*` does, which `A` undoes. A stopped session is archived with no question. One that
//! runs is stopped first, as `k` stops it, only when its screen is settled and Claude Code's
//! registry says its process is idle, read on `oso-session-idle`; that stop asks the person, or
//! goes without asking as the person's configuration lets a start. Its worktree, branch and
//! images stay. The registry is read on a thread of its own, and the stop is the watch's
//! (`App::stop_quietly`), never these files'.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here touches the disk.
//!
//! - Every rule is looked at again on the UI thread before the stop and the archive: once
//!   the registry was read, and once more after the person's `y`, which reads it again.
//! - One look at a time per caller, each answered within [`LOOK_WITHIN`].
//! - A session shown in the selected window is never stopped without asking: the person may be
//!   about to type in it.
//! - The images of an archived session stay until the week's sweep (ADR 0142), as the person's
//!   `A` brings the row back but not a deleted image.

use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde_json::json;

use super::super::super::{App, UserEvent};
use super::super::worktrees::{hosted_state, settled};
use super::asks::Plan;
use super::question::details;
use super::space::Role;
use super::{Caller, Pending, shown};
use crate::sessions::hosted::Run;
use crate::sessions::identity::SessionId;
use crate::sessions::registry_look::Found;
use crate::sessions::tools::{self as rules, Archivable, Archive, Runs, Tool};
use crate::sessions::worker::SessionKey;

/// How long a look at the registry may take before its call is answered that it did not end:
/// one small file, unless the registry's disk hangs.
const LOOK_WITHIN: Duration = Duration::from_secs(5);

/// What `?` shows of an archive that stops a session, after the question.
const ARCHIVE_DETAILS: &str = "oso hangs the session up, as k in *Spaces* does, and archives its \
     row; A under Archived brings it back, and session-resume takes up its conversation. Its \
     worktree and branch are kept.";

/// A session an agent's archive stops first, while its registry entry is read and its question
/// waits.
#[derive(Debug)]
pub(in crate::app) struct Archiving {
    /// The session to stop and archive, and its process when the call came.
    key: SessionKey,
    pid: u32,
    id: SessionId,
    /// The session that asked.
    caller: SessionKey,
}

/// A look at the registry that runs, with the call that waits for it.
#[derive(Debug)]
pub(in crate::app) struct Looking {
    pub(super) pending: Pending,
    archiving: Archiving,
    /// It follows the person's `y`: the stop comes next, with no question.
    pub(super) allowed: bool,
    started: Instant,
}

impl App {
    /// `session_archive` of `session` from `caller`.
    pub(super) fn tool_session_archive(
        &mut self,
        pending: Pending,
        tool: &Tool,
        caller: &Caller,
        session: &str,
    ) {
        let (key, id) = match self.find_session(Some(caller), session) {
            Ok(found) => (found.key, found.id.clone()),
            Err(why) => return pending.answer(Err(why)),
        };
        let archive = match self.archivable_by(caller, key) {
            Ok(archive) => archive,
            Err(why) => return pending.answer(Err(why.into())),
        };
        let asks = rules::archive_asks(archive, tool);
        if let Archive::StopFirst { .. } = archive
            && (self.hosting.tools.archive_looks.values())
                .any(|look| look.archiving.caller == caller.key)
        {
            return pending.answer(Err(
                "another archive of yours waits for Claude Code's registry; try again".into(),
            ));
        }
        if let Err(why) = self.admit_asking(tool, asks, caller, id.as_str()) {
            return pending.answer(Err(why));
        }
        match archive {
            Archive::Row => self.archive_row(pending, key, &caller.name, false),
            Archive::StopFirst { pid } => {
                let archiving = Archiving {
                    key,
                    pid,
                    id,
                    caller: caller.key,
                };
                self.look_if_idle(pending, archiving, false);
            }
        }
    }

    /// Whether session `key` may be archived by `caller`, and how, by [`rules::archivable`],
    /// over what the caller reaches of it (`App::reach_over`): an orchestrator, current or
    /// earlier, is never archived by an agent.
    fn archivable_by(&self, caller: &Caller, key: SessionKey) -> Result<Archive, &'static str> {
        let session = self
            .hosting
            .table()
            .get(key)
            .ok_or("that session is gone")?;
        let reach = self.reach_over(Some(caller), session);
        let runs = match session.run {
            Run::Stopped | Run::Failed => Runs::No,
            Run::Starting => Runs::Starting,
            Run::Running { .. } if self.watch.is_hung_up(key) => Runs::Stopping,
            Run::Running { child } => Runs::Yes {
                pid: child,
                settled: settled(hosted_state(session, &self.watch)),
            },
        };
        rules::archivable(Archivable {
            caller: caller.key == key,
            orchestrator: reach.recorded
                || matches!(reach.role, Some(Role::Orchestrator | Role::Former)),
            pinned: session.pinned,
            archived: session.archived,
            of_callers_space: reach.of_callers_space,
            callers_child: reach.callers_child,
            runs,
        })
    }

    /// Whether `archiving` still holds as it did when the call came: the same session and the
    /// same caller, whose connection `pending` still is, and the rules still stop the same
    /// process first.
    fn still_stops_first(
        &self,
        pending: &Pending,
        archiving: &Archiving,
    ) -> Result<Caller, String> {
        let table = self.hosting.table();
        if table
            .get(archiving.key)
            .is_none_or(|s| s.id != archiving.id)
        {
            return Err("that session is gone".into());
        }
        let caller = (table.get(archiving.caller))
            .filter(|_| {
                table.by_connection(pending.connection).map(|s| s.key) == Some(archiving.caller)
            })
            .map(|session| self.caller_of(session))
            .ok_or("your session is gone")?;
        match self.archivable_by(&caller, archiving.key)? {
            Archive::StopFirst { pid } if pid == archiving.pid => Ok(caller),
            Archive::StopFirst { .. } => Err("that session runs as another process now".into()),
            Archive::Row => Err("that session stopped meanwhile; call again to archive it".into()),
        }
    }

    /// Reads `archiving`'s registry entry on `oso-session-idle`; `allowed` once the person
    /// said `y`.
    fn look_if_idle(&mut self, pending: Pending, archiving: Archiving, allowed: bool) {
        let tools = &mut self.hosting.tools;
        tools.archive_token += 1;
        let token = tools.archive_token;
        let pid = archiving.pid;
        let waker = Mutex::new(self.waker.clone());
        let spawned = crate::sessions::registry_look::spawn_look(pid, move |found| {
            (waker.lock().unwrap_or_else(PoisonError::into_inner))
                .send(UserEvent::ArchiveLooked(token, Box::new(found)));
        });
        if let Err(e) = spawned {
            log::warn!("cannot read the registry for an archive: {e}");
            return pending.answer(Err("oso cannot read Claude Code's registry now".into()));
        }
        self.wake_after(LOOK_WITHIN);
        self.hosting.tools.archive_looks.insert(
            token,
            Looking {
                pending,
                archiving,
                allowed,
                started: Instant::now(),
            },
        );
    }

    /// Answers the looks that took longer than [`LOOK_WITHIN`]: nothing is stopped, and what
    /// their thread finds later is dropped.
    pub(super) fn end_slow_archive_looks(&mut self) {
        let looks = &mut self.hosting.tools.archive_looks;
        let slow: Vec<u64> = (looks.iter())
            .filter(|(_, look)| look.started.elapsed() >= LOOK_WITHIN)
            .map(|(&token, _)| token)
            .collect();
        for token in slow {
            if let Some(look) = self.hosting.tools.archive_looks.remove(&token) {
                self.agent_log("session_archive: kept running");
                look.pending.answer(Err(
                    "Claude Code's registry did not answer in time; nothing was stopped".into(),
                ));
            }
        }
    }

    /// The registry entry of look `token` was read: the rules are looked at again, then the
    /// person is asked, or the session is stopped and archived.
    pub(in crate::app) fn archive_looked(&mut self, token: u64, found: Found) {
        let Some(looking) = self.hosting.tools.archive_looks.remove(&token) else {
            return;
        };
        let Looking {
            mut pending,
            archiving,
            allowed,
            ..
        } = looking;
        let caller = match self.still_stops_first(&pending, &archiving) {
            Ok(caller) => caller,
            Err(why) => return pending.answer(Err(why)),
        };
        let found = found.as_ref().map(|(entry, live)| (entry, *live));
        if let Err(not) = rules::registry_idle(archiving.pid, found) {
            self.agent_log("session_archive: kept running");
            return pending.answer(Err(not.why().into()));
        }
        if allowed {
            return self.stop_and_archive(pending, &archiving, &caller);
        }
        let Some(session) = self.hosting.table().get(archiving.key) else {
            return pending.answer(Err("that session is gone".into()));
        };
        let (name, place) = (session.name.clone(), shown(&session.directory));
        let in_sight = session
            .buffer
            .is_some_and(|b| b == self.editor.tiled().0.buffer());
        if !in_sight && rules::stops_without_asking(self.agent_starts_allowed(), caller.origin) {
            pending.unasked = true;
            self.agent_log("session_archive: stopping without asking");
            return self.stop_and_archive(pending, &archiving, &caller);
        }
        if self.agent_question_waits() {
            return pending.answer(Err(super::QUESTION_WAITS.into()));
        }
        let label = rules::archive_question(&caller.name, &name, &place);
        let details = details(&caller.name, &label, ARCHIVE_DETAILS);
        let target = archiving.id.as_str().to_owned();
        self.queue_question(
            pending,
            (caller.id.clone(), target),
            (label, details),
            Plan::Archive(archiving),
        );
    }

    /// The person's `y`: the registry is read once more, as the question may have waited, and
    /// the rules looked at again then.
    pub(super) fn carry_out_archive(&mut self, pending: Pending, archiving: Archiving) {
        if let Err(why) = self.still_stops_first(&pending, &archiving) {
            return pending.answer(Err(why));
        }
        self.look_if_idle(pending, archiving, true);
    }

    /// Stops session `archiving`, as `k` does, and archives its row.
    fn stop_and_archive(&mut self, pending: Pending, archiving: &Archiving, caller: &Caller) {
        if !self.stop_quietly(archiving.key) {
            return pending.answer(Err("that session could not be stopped".into()));
        }
        self.archive_row(pending, archiving.key, &caller.name, true);
    }

    /// Archives session `key`'s row for the agent `by`, `stopped` first or not, says so, and
    /// answers the call. Its images stay, to go with the week's sweep (ADR 0142).
    fn archive_row(&mut self, pending: Pending, key: SessionKey, by: &str, stopped: bool) {
        let Some(session) = self.hosting.table.get_mut(key) else {
            return pending.answer(Err("that session is gone".into()));
        };
        session.archived = true;
        let (id, name) = (session.id.as_str().to_owned(), session.name.clone());
        self.remember_sessions();
        let unasked = stopped && pending.unasked;
        let asked = stopped && !pending.unasked;
        let said = rules::archived_said(by, &name, stopped, unasked);
        self.editor.message(format!("Agent: {said}"));
        self.agent_log("session_archive: archived");
        self.request_redraw();
        let note = unasked.then_some("stopped without asking, as the person's config.toml allows");
        pending.answer(Ok(json!({
            "id": id,
            "name": name,
            "archived": true,
            "stopped": stopped,
            "asked": asked,
            "note": note,
        })));
    }
}
