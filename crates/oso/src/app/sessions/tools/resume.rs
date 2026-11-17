//! `session_resume` (ADR 0153): a space's orchestrator resumes a stopped session of its space,
//! as the person's `session-resume` does, in the session's own buffer, under its name and in
//! its directory. It asks the person, or goes without asking by ADR 0135's rules, the session
//! then counting toward the caller's limit while it runs. A conversation that runs in another
//! process is refused, never forked.
//!
//! Must not depend on: key presses or how a frame is drawn. The resume itself is the hosting
//! code's, on the session's start thread.

use std::path::Path;

use super::super::super::App;
use super::asks::{Plan, Words};
use super::question::details;
use super::space::in_place;
use super::{Caller, Pending, shown};
use crate::sessions::hosted::Hosted;
use crate::sessions::identity::SessionId;
use crate::sessions::spaces::SpaceId;
use crate::sessions::tools::{self as rules, Resumable, Tool};
use crate::sessions::worker::SessionKey;

/// What `?` shows of a resume, after the question.
const RESUME_DETAILS: &str =
    "The session takes up its own conversation in its own buffer; oso sends it no prompt.";

/// What the person's `y`, or a resume without asking, runs.
#[derive(Debug)]
pub(in crate::app) struct Resuming {
    /// The session to resume.
    key: SessionKey,
    /// The orchestrator that asked.
    caller: SessionKey,
    caller_id: SessionId,
    /// Its space, where the session must still be at the person's `y`.
    space: SpaceId,
}

impl App {
    /// `session_resume` of `session` from `caller`.
    pub(super) fn tool_session_resume(
        &mut self,
        pending: Pending,
        tool: &Tool,
        caller: &Caller,
        session: &str,
    ) {
        let Some(space) = (caller.space.as_ref()).filter(|_| caller.orchestrator()) else {
            return pending.answer(Err(
                "only a space's orchestrator resumes sessions; the person resumes this one".into(),
            ));
        };
        let space = space.id.clone();
        let (key, id, name, directory, trusted) = match self.find_session(Some(caller), session) {
            Ok(found) => {
                let trusted = self.trusted_where_it_runs(found);
                let (key, id) = (found.key, found.id.clone());
                (
                    key,
                    id,
                    found.name.clone(),
                    found.directory.clone(),
                    trusted,
                )
            }
            Err(why) => return pending.answer(Err(why)),
        };
        if let Err(why) = self.resumable(caller.key, key) {
            return pending.answer(Err(why.into()));
        }
        if let Err(why) = self.admit_tool(tool, caller, id.as_str()) {
            return pending.answer(Err(why));
        }
        let place = shown(&directory);
        let label = rules::resume_question(&caller.name, &name, &place);
        let said = rules::resume_started(&caller.name, &name, &place);
        let words = Words(
            label.clone(),
            details(&caller.name, &label, RESUME_DETAILS),
            said,
        );
        let resuming = Resuming {
            key,
            caller: caller.key,
            caller_id: caller.id.clone(),
            space,
        };
        let target = id.as_str().to_owned();
        self.ask_or_start(
            pending,
            caller,
            (target, trusted),
            words,
            Plan::Resume(resuming),
        );
    }

    /// Whether `session`'s project is trusted now, by an open buffer of it, its own among them,
    /// as for a start in the caller's own directory (ADR 0135, Q4), and it runs in that project:
    /// a record whose root and directory part ways never resumes without asking.
    fn trusted_where_it_runs(&self, session: &Hosted) -> bool {
        let under_root =
            (session.root.as_deref()).is_some_and(|root| in_place(&session.directory, root));
        under_root && self.caller_trusted_now(&Caller::of(session))
    }

    /// Whether session `key` may be resumed by the session `caller`, by
    /// [`rules::resumable`].
    fn resumable(&self, caller: SessionKey, key: SessionKey) -> Result<(), &'static str> {
        let session = self
            .hosting
            .table()
            .get(key)
            .ok_or("that session is gone")?;
        rules::resumable(Resumable {
            caller: key == caller,
            runs: session.runs(),
            archived: session.archived,
            // An orchestrator of the space, current or earlier, sees its space only.
            orchestrator: self.caller_of(session).scoped(),
            buffer: (session.buffer).is_some_and(|id| self.editor.buffer(id).is_some()),
        })
    }

    /// Whether `resuming`'s rules still hold, as they are at the person's `y`: its caller is
    /// still its space's orchestrator, the session still of that space and still resumable.
    fn still_resumable(&self, resuming: &Resuming) -> Result<(), &'static str> {
        let table = self.hosting.table();
        let orchestrates = table.get(resuming.caller).is_some_and(|caller| {
            let caller = self.caller_of(caller);
            caller.orchestrator() && caller.space.is_some_and(|at| at.id == resuming.space)
        });
        if !orchestrates {
            return Err("your session is no longer its space's orchestrator");
        }
        let in_space = (table.get(resuming.key))
            .is_some_and(|session| self.session_in_space(session, &resuming.space));
        if !in_space {
            return Err("that session is no longer of your space");
        }
        self.resumable(resuming.caller, resuming.key)
    }

    /// The person's `y`, or a resume without asking: the session resumes behind what the
    /// person is doing, if it still may, and the call's reply waits for it to run. `said`, for
    /// a resume without asking, is said once it is under way.
    pub(super) fn carry_out_resume(
        &mut self,
        pending: Pending,
        resuming: Resuming,
        said: Option<String>,
    ) {
        if let Err(why) = self.still_resumable(&resuming) {
            return pending.answer(Err(why.into()));
        }
        let key = resuming.key;
        match self.resume_hosted(key, false) {
            Ok(()) => {
                let tools = &mut self.hosting.tools;
                if pending.unasked {
                    // Only running sessions count, so only theirs are kept.
                    let table = self.hosting.table.all();
                    tools
                        .resumed
                        .retain(|kept, _| table.iter().any(|s| s.key == *kept && s.runs()));
                    tools.resumed.insert(key, resuming.caller_id);
                } else {
                    tools.resumed.remove(&key);
                }
                tools.starts.insert(key, pending);
                if let Some(said) = said {
                    self.editor.message(format!("Agent: {said}"));
                }
            }
            Err(why) => pending.answer(Err(why)),
        }
    }

    /// The start thread found the conversation of session `key` live in another process while
    /// a tool's call resumed it: the call is refused, as no agent forks a conversation (ADR
    /// 0153). Returns whether a call waited for it; if not, the person's resume offers a fork.
    pub(in crate::app) fn session_tool_live(
        &mut self,
        key: SessionKey,
        pid: Option<u32>,
        directory: Option<&Path>,
    ) -> bool {
        let Some(pending) = self.hosting.tools.starts.remove(&key) else {
            return false;
        };
        self.forget_agent_resume(key);
        let by = pid.map_or_else(
            || "another process".to_owned(),
            |pid| format!("process {pid}"),
        );
        let place = directory.map_or_else(
            || "a directory oso cannot tell".to_owned(),
            |directory| rules::quoted(&shown(directory)),
        );
        let why = format!(
            "its conversation runs in {by} in {place}: stop it there first; oso forks a \
             conversation only for the person"
        );
        self.agent_log(&format!("{}: not started", pending.tool));
        let name = (self.hosting.table.get(key))
            .map_or_else(|| "a session".to_owned(), |s| rules::quoted(&s.name));
        self.editor
            .message(format!("Agent: session {name} not resumed: {why}"));
        pending.answer(Err(why));
        true
    }

    /// Session `key` is no agent's resume any more: the person resumed it, or it did not start.
    pub(in crate::app) fn forget_agent_resume(&mut self, key: SessionKey) {
        self.hosting.tools.resumed.remove(&key);
    }
}
