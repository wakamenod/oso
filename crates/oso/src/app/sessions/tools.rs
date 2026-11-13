//! The agents' session tools in the app (ADR 0121, "The bridge and the channel"; step 6):
//! who calls, what each call may do, and carrying it out with what the sessions and worktree
//! code already has. The rules are `sessions::tools`'; this only applies them.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here touches the disk or
//! starts a process: git runs on the worktree jobs, and a session starts on its start thread,
//! only after the person's `y`.
//!
//! - A connection proven to be a hosted session's bridge calls as that session. Any other
//!   connection may only read, within the selected window's project (the author's choice).
//! - A tool that asks puts its question to the person once they are free, as the connection's
//!   own question waits, and its reply waits for the answer and for what the answer starts.
//! - A call that changes something is counted, and refused past the loop guard or within a
//!   minute of the person's `n` to the same call.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::Instant;

use oso_core::buffer::BufferId;
use serde_json::{Value, json};

use super::super::App;
use crate::channel::protocol::sessions::Call;
use crate::channel::{Connection, Reply};
use crate::sessions::hosted::Hosted;
use crate::sessions::identity::SessionId;
use crate::sessions::tools::{self as rules, Asks, Guard, Tool};
use crate::sessions::worker::SessionKey;
use crate::sessions::worktree::jobs::{Report, Trust};
use crate::sessions::worktree::porcelain::printable;

mod asks;
mod reads;
mod remove;

pub(in crate::app) use asks::Question;
use reads::Listing;

/// What the app keeps of the agents' session tools.
#[derive(Debug, Default)]
pub(in crate::app) struct Tools {
    guard: Guard,
    /// Calls whose reply waits for a worktree job, by the job's token.
    jobs: HashMap<u64, Job>,
    /// Calls whose reply waits for a session to start, by its key.
    starts: HashMap<SessionKey, Pending>,
    /// The question of a tool that asks, before and while it is asked.
    question: Option<Question>,
    /// When each caller's recent worktree listings started, for their limit.
    listings: HashMap<Lister, Vec<Instant>>,
    /// `*Agent request*`, while it is shown for a question.
    details: Option<BufferId>,
}

/// A call whose reply waits.
#[derive(Debug)]
pub(in crate::app) struct Pending {
    connection: Connection,
    reply: Sender<Reply>,
    tool: &'static str,
}

impl Pending {
    fn answer(self, reply: Reply) {
        let _ = self.reply.send(reply);
    }
}

/// What a worktree job's report is for.
#[derive(Debug)]
enum Job {
    /// Looking before the person is asked.
    Prepared(asks::Preparing),
    /// Making a worktree after the person's `y`; its session starts next.
    Creating(asks::Creating),
    Listing(Listing),
    Removing {
        pending: Pending,
        path: PathBuf,
    },
}

impl Job {
    fn connection(&self) -> Connection {
        match self {
            Self::Prepared(preparing) => preparing.pending.connection,
            Self::Creating(creating) => creating.pending.connection,
            Self::Listing(listing) => listing.pending.connection,
            Self::Removing { pending, .. } => pending.connection,
        }
    }
}

/// Who a worktree listing is for, for its limit.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Lister {
    Session(SessionId),
    Outside(Connection),
}

/// The hosted session a call comes from, as far as the call needs it.
#[derive(Clone, Debug)]
pub(in crate::app) struct Caller {
    key: SessionKey,
    id: SessionId,
    name: String,
    directory: PathBuf,
    root: Option<PathBuf>,
    runs: bool,
}

impl Caller {
    fn of(session: &Hosted) -> Self {
        Self {
            key: session.key,
            id: session.id.clone(),
            name: session.name.clone(),
            directory: session.directory.clone(),
            root: session.root.clone(),
            runs: session.runs(),
        }
    }
}

impl App {
    /// A session tool's call from `connection`, whose reply goes to `reply` now or once what
    /// it waits for has happened.
    pub(in crate::app) fn session_tool(
        &mut self,
        connection: Connection,
        call: Call,
        reply: Sender<Reply>,
    ) {
        let Some(&tool) = rules::tool(call.name()) else {
            let _ = reply.send(Err("no such session tool".into()));
            return;
        };
        let caller = self.hosting.table.by_connection(connection).map(Caller::of);
        let pending = Pending {
            connection,
            reply,
            tool: tool.name,
        };
        if caller.is_none() && !tool.outside {
            return pending.answer(Err(format!(
                "{} is only for a session oso started; this agent may read sessions and worktrees",
                tool.name
            )));
        }
        let who = caller
            .as_ref()
            .map_or_else(|| "an agent".to_owned(), |c| printable(&c.name, 40));
        self.agent_log(&format!("{}: from {who}", tool.name));
        match (call, caller) {
            (Call::List { offset }, caller) => {
                let listed = self.session_list(caller.as_ref(), offset);
                pending.answer(listed);
            }
            (Call::State { session }, caller) => {
                let state = self.session_state(caller.as_ref(), &session);
                pending.answer(state);
            }
            (Call::WorktreeList, caller) => self.tool_worktree_list(pending, caller),
            (Call::WorktreeRemove { path }, Some(caller)) => {
                self.tool_worktree_remove(pending, &tool, &caller, path);
            }
            (call @ (Call::StartWorktree { .. } | Call::Start { .. }), Some(caller)) => {
                self.tool_start(pending, &tool, &caller, call);
            }
            (_, None) => pending.answer(Err("this tool is only for a session oso started".into())),
        }
    }

    /// Counts a call of `tool` on `target` from `caller` toward the loop guard, or says why
    /// it is refused: past the guard, within a minute of the person's `n` to the same call, or,
    /// for a tool that asks, while another agent's question waits.
    fn admit_tool(&mut self, tool: &Tool, caller: &Caller, target: &str) -> Result<(), String> {
        if tool.asks == Asks::Each && (self.hosting.tools.question.is_some() || self.agent_asks()) {
            return Err("a question an agent caused waits for the person; try again".into());
        }
        if tool.asks == Asks::No
            && let Some(why) = self.agent_waits()
        {
            return Err(why.into());
        }
        self.hosting
            .tools
            .guard
            .admit(&caller.id, tool.name, target, Instant::now())
            .map_err(|refusal| refusal.why().to_owned())
    }

    /// The trust `caller`'s worktree jobs run under: the one it started with, else that of
    /// an open project whose root is its own, if trusted.
    fn caller_trust(&self, caller: &Caller) -> Result<Trust, String> {
        let started = self.hosting.trusts.get(&caller.key).cloned();
        let root = started.or_else(|| {
            let root = caller.root.as_deref()?;
            self.editor
                .buffers()
                .filter_map(|(_, buffer)| buffer.project())
                .find(|&id| self.editor.project(id).is_some_and(|p| p.root() == root))
                .and_then(|id| crate::review::TrustedRoot::of(&self.editor, id))
        });
        let root = root.ok_or(
            "your session's project is not trusted; the person trusts it with trust-project",
        )?;
        Ok(Trust {
            root,
            trusted: self.trusted_list(),
        })
    }

    /// The person's input reached the terminal of the session `buffer` holds: its agent may
    /// make as many calls again (ADR 0121, the author's choice: only this resets the guard).
    #[expect(dead_code, reason = "ADR 0120 step 4a's terminal input calls it")]
    pub(in crate::app) fn person_typed_in_session(&mut self, buffer: BufferId) {
        if let Some(session) = self.hosting.table.by_buffer(buffer) {
            let id = session.id.clone();
            self.hosting.tools.guard.person_typed(&id);
        }
    }

    /// A report of a worktree job, taken if a tool's call waits for it, else given back.
    pub(in crate::app) fn session_tool_report(&mut self, report: Report) -> Option<Report> {
        let token = match &report {
            Report::Prepared { token, .. }
            | Report::Step { token, .. }
            | Report::Created { token, .. }
            | Report::Listed { token, .. }
            | Report::Row { token, .. }
            | Report::Merged { token, .. }
            | Report::ListEnded { token }
            | Report::Removed { token, .. } => *token,
            Report::Branches { .. } | Report::Checked { .. } => return Some(report),
        };
        let Some(job) = self.hosting.tools.jobs.remove(&token) else {
            return Some(report);
        };
        match job {
            Job::Prepared(preparing) => self.tool_prepared(preparing, report),
            Job::Creating(creating) => self.tool_creating(token, creating, report),
            Job::Listing(listing) => self.tool_listing(token, listing, report),
            Job::Removing { pending, path } => self.tool_removed(pending, &path, report),
        }
        None
    }

    /// The session `key` that a tool's call asked for started, or did not.
    pub(in crate::app) fn session_tool_started(
        &mut self,
        key: SessionKey,
        started: Result<(), String>,
    ) {
        let Some(pending) = self.hosting.tools.starts.remove(&key) else {
            return;
        };
        let session = self.hosting.table.get(key);
        let reply = match (started, session) {
            (Ok(()), Some(session)) => Ok(json!({
                "session": { "id": session.id.as_str(), "name": session.name },
                "directory": session.directory.display().to_string(),
            })),
            (Ok(()), None) => Err("the session started, and ended at once".into()),
            (Err(why), _) => Err(format!("the session did not start: {why}")),
        };
        self.agent_log(&format!(
            "{}: {}",
            pending.tool,
            if reply.is_ok() {
                "started"
            } else {
                "not started"
            }
        ));
        pending.answer(reply);
    }

    /// The connection `connection` went, or every one did when it is `None`: its calls get no
    /// reply, its question closes, and nothing it asked for and the person has not allowed
    /// runs. What the person's `y` started goes on, as the person's.
    pub(in crate::app) fn session_tools_gone(&mut self, connection: Option<Connection>) {
        let ours = |of: Connection| connection.is_none_or(|gone| gone == of);
        let tools = &mut self.hosting.tools;
        tools
            .jobs
            .retain(|_, job| matches!(job, Job::Creating(_)) || !ours(job.connection()));
        let closes = tools
            .question
            .as_ref()
            .is_some_and(|question| ours(question.pending.connection));
        if closes {
            self.close_tool_question();
        }
    }

    /// After each event: asks a tool's question once the person is free, takes the answer, and
    /// ends listings that ran too long.
    pub(in crate::app) fn sync_session_tools(&mut self) {
        self.ask_about_a_tool();
        self.end_slow_listings();
    }
}

/// A path as a tool's reply gives it.
fn shown(path: &Path) -> String {
    path.display().to_string()
}

/// A reply for a session, as `session_list` and `session_state` give it.
fn session_json(session: &Hosted, caller: Option<&SessionId>, now_ms: u64) -> Value {
    use crate::sessions::hosted::Run;
    // The taxonomy of states comes with ADR 0121 step 3; until then, the process's.
    let state = match session.run {
        Run::Starting => "starting",
        Run::Running { .. } => "running",
        Run::Stopped => "stopped",
        Run::Failed => "failed",
    };
    json!({
        "id": session.id.as_str(),
        "name": session.name,
        // The name Claude Code's own SendMessage and ListAgents know it by.
        "messaging_name": session.name,
        "directory": shown(&session.directory),
        "project": session.root.as_deref().map(shown),
        "state": state,
        "age_s": now_ms.saturating_sub(session.changed) / 1000,
        "pinned": session.pinned,
        "archived": session.archived,
        "parent": session.parent.as_ref().map(SessionId::as_str),
        "external": false,
        "you": caller == Some(&session.id),
    })
}
