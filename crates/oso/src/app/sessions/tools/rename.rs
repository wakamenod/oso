//! Renaming a hosted session (ADR 0155): the person's `session-rename`, on a row of `*Spaces*`
//! or in a session's terminal, and an agent's `session_rename`. Both change the name in oso's
//! record and the terminal buffer's, `*claude: NAME*`, at once. A running `claude` keeps the
//! name Claude Code's `SendMessage` knows it by until its next resume passes the new one with
//! `--name`, and what is said names it.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here touches the disk or
//! starts a process; the record is written by the saver, as for a pin.
//!
//! - A space's current orchestrator is never renamed: its name is the space's, which the
//!   space's sessions message (ADR 0136, Q4).
//! - An agent renames itself, the sessions it started and, as its space's current
//!   orchestrator, that space's sessions; never an orchestrator, current or earlier.
//! - No name another session oso knows of has, hosted, being started or another program's, nor
//!   the name Claude Code knows one by, nor a space's or an earlier orchestrator's of one,
//!   ASCII case aside.

use serde_json::json;

use super::super::super::App;
use super::{Caller, Pending};
use crate::commands::agent_sessions::RENAME_TO;
use crate::sessions::identity::new_name;
use crate::sessions::tools::{self as rules, Renamable, Tool};
use crate::sessions::worker::SessionKey;

/// What a rename did, for its reply.
struct Renamed {
    old: String,
    messaging: String,
    note: Option<String>,
}

impl App {
    /// `session-rename` on session `key`: opens the prompt of `session-rename-to` holding its
    /// name now, `M-p` giving back an empty one, unless the rules refuse it.
    pub(in crate::app) fn ask_session_rename(&mut self, key: SessionKey) {
        if let Err(why) = self.renamable_by(None, key) {
            return self.editor.message(why);
        }
        let Some(name) = self.hosting.table().get(key).map(|s| s.name.clone()) else {
            return;
        };
        let Some(prompt) = self.open_host_prompt(RENAME_TO) else {
            return self.editor.message("Cannot ask for the name now");
        };
        self.editor.offer_prompt_default(prompt, name);
        let _ = self.editor.walk_history(false);
        self.hosting.naming.set_asked(prompt, key);
    }

    /// `session-rename-to`'s answer `typed`: the session its prompt was opened for takes it,
    /// cleaned, if the rules still let it.
    pub(in crate::app) fn session_rename_answered(&mut self, typed: &str) {
        const NONE_ASKED: &str = "No rename was asked: session-rename asks";
        let Some((prompt, key)) = self.hosting.naming.take_asked() else {
            return self.editor.message(NONE_ASKED);
        };
        if self.editor.prompts().iter().any(|p| p.id() == prompt) {
            return self.editor.message(NONE_ASKED);
        }
        let checked = (self.renamable_by(None, key).map_err(str::to_owned))
            .and_then(|()| new_name(typed, false))
            .and_then(|name| self.free_name(key, &name).map(|()| name));
        match checked {
            Ok(name) => {
                self.rename_session(key, &name, None);
            }
            Err(why) => self.editor.message(why),
        }
    }

    /// Forgets a rename whose prompt closed without an answer, so that a later answer cannot
    /// name its session.
    pub(in crate::app) fn drop_unanswered_rename(&mut self) {
        if let Some((prompt, _)) = self.hosting.naming.asked()
            && !self.editor.prompts().iter().any(|p| p.id() == prompt)
        {
            self.hosting.naming.take_asked();
        }
    }

    /// `session_rename` of `session` to `name`, from `caller`: no question, as it is metadata
    /// only (ADR 0121's table), but counted by the loop guard before the name is looked up, so
    /// that probing which names are taken counts too.
    pub(super) fn tool_session_rename(
        &mut self,
        pending: Pending,
        tool: &Tool,
        caller: &Caller,
        (session, name): (&str, &str),
    ) {
        let (key, id) = match self.find_session(Some(caller), session) {
            Ok(found) => (found.key, found.id.clone()),
            Err(why) => return pending.answer(Err(why)),
        };
        let checked = (self.renamable_by(Some(caller), key).map_err(str::to_owned))
            .and_then(|()| new_name(name, true))
            .and_then(|name| self.admit_tool(tool, caller, id.as_str()).map(|()| name))
            .and_then(|name| self.free_name(key, &name).map(|()| name));
        let name = match checked {
            Ok(name) => name,
            Err(why) => return pending.answer(Err(why)),
        };
        let Some(renamed) = self.rename_session(key, &name, Some(&caller.name)) else {
            return pending.answer(Err("that session is gone".into()));
        };
        self.agent_log("session_rename: renamed");
        pending.answer(Ok(json!({
            "id": id.as_str(),
            "name": name,
            "was": renamed.old,
            "messaging_name": renamed.messaging,
            "note": renamed.note,
        })));
    }

    /// Whether session `key` may be renamed by `caller`, an agent's session, or by the person,
    /// by [`rules::renamable`]. Whether it is a space's orchestrator is the record's word,
    /// failing closed: also one of a closed space, or one whose directory no longer passes the
    /// checks that give it its part. Its part decides only whether a caller reaches it.
    fn renamable_by(&self, caller: Option<&Caller>, key: SessionKey) -> Result<(), &'static str> {
        let session = self
            .hosting
            .table()
            .get(key)
            .ok_or("that session is gone")?;
        let spaces = self.watch.spaces();
        let recorded = spaces.orchestrated_by(&session.id).is_some();
        let role = self.caller_of(session).space.map(|space| space.role);
        let of_callers_space = caller.is_some_and(|caller| {
            caller.orchestrator()
                && (caller.space.as_ref()).is_some_and(|at| self.session_in_space(session, &at.id))
        });
        rules::renamable(Renamable {
            agent: caller.is_some(),
            caller: caller.is_some_and(|caller| caller.key == key),
            orchestrator: recorded || role == Some(super::space::Role::Orchestrator),
            former: role == Some(super::space::Role::Former),
            of_callers_space,
            callers_child: caller.is_some_and(|caller| session.parent.as_ref() == Some(&caller.id)),
        })
    }

    /// Whether `name` is free for session `key`, by [`rules::rename_clash`]: no other session
    /// oso hosts or is starting, no other program's session, no name another of oso's running
    /// sessions was started with or the registry gives it, and no space's.
    ///
    /// # Errors
    ///
    /// Why it is taken, or that it is the session's name already.
    fn free_name(&self, key: SessionKey, name: &str) -> Result<(), String> {
        let table = self.hosting.table();
        if table.get(key).is_some_and(|session| session.name == name) {
            return Err(format!(
                "That session is named {} already",
                rules::quoted(name)
            ));
        }
        let others = || table.iter().filter(|session| session.key != key);
        let answered = others()
            .filter(|session| session.runs())
            .flat_map(|session| {
                let registry = self.hosting.naming.messaging_name(session.key);
                registry.into_iter().chain(session.launched_as.as_deref())
            });
        let sessions = (others().map(|session| session.name.as_str()))
            .chain(answered)
            .chain(table.held_names())
            .chain(self.watch.external_names());
        let spaces: Vec<&str> = self.watch.spaces().iter().map(|s| s.name()).collect();
        match rules::rename_clash(name, sessions, spaces.iter().copied()) {
            Some(why) => Err(format!("{}: {why}", rules::quoted(name))),
            None => Ok(()),
        }
    }

    /// Renames session `key` to `name`, checked already, in the record and its buffer, and
    /// says so, `by` the agent of that name or by the person. A running session keeps the name
    /// its `claude` was started with as the one it answers to until its next start.
    fn rename_session(&mut self, key: SessionKey, name: &str, by: Option<&str>) -> Option<Renamed> {
        // What its claude answers to until it is resumed, and its children were told.
        let messaging = self.messaging_name(self.hosting.table().get(key)?);
        let session = self.hosting.table.get_mut(key)?;
        let old = std::mem::replace(&mut session.name, name.to_owned());
        let runs = session.runs();
        if runs && session.launched_as.is_none() {
            session.launched_as = Some(old.clone());
        }
        if session.launched_as.as_deref() == Some(name) {
            session.launched_as = None;
        }
        let (buffer, id) = (session.buffer, session.id.clone());
        if let Some(buffer) = buffer {
            self.editor
                .rename_terminal_buffer(buffer, &super::super::hosting::buffer_name(name));
        }
        self.remember_sessions();
        let children = (self.hosting.table().iter())
            .filter(|s| !s.archived && s.parent.as_ref() == Some(&id))
            .count();
        let note = rules::rename_note(&messaging, name, runs, children);
        let said = rules::renamed_said(by, &old, name);
        let said = match &note {
            Some(note) => format!("{said}; {note}"),
            None => said,
        };
        let said = match by {
            Some(_) => format!("Agent: {said}"),
            None => said,
        };
        self.editor.message(said);
        self.request_redraw();
        let messaging =
            (self.hosting.table().get(key)).map_or(messaging, |s| self.messaging_name(s));
        Some(Renamed {
            old,
            messaging,
            note,
        })
    }
}
