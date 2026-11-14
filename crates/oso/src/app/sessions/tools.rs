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
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Sender;
use std::time::Instant;

use oso_core::buffer::BufferId;
use serde_json::json;

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
    /// The one agent listing that runs, by its job's token, and what stops it.
    listing: Option<(u64, Arc<AtomicBool>)>,
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
    Prepared(Box<asks::Preparing>),
    /// Making a worktree after the person's `y`; its session starts next.
    Creating(asks::Creating),
    Listing(Listing),
    Removing(remove::Removing),
}

impl Job {
    fn connection(&self) -> Connection {
        match self {
            Self::Prepared(preparing) => preparing.pending.connection,
            Self::Creating(creating) => creating.pending.connection,
            Self::Listing(listing) => listing.pending.connection,
            Self::Removing(removing) => removing.connection,
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
        if tool.read_only() {
            // Reads may come many times a second, as `state` does.
            self.agent_log_often(tool.name);
        } else {
            let who = caller
                .as_ref()
                .map_or_else(|| "an agent".to_owned(), |c| printable(&c.name, 40));
            self.agent_log(&format!("{}: from {who}", tool.name));
        }
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
            (
                Call::StartWorktree {
                    branch,
                    base,
                    brief,
                    name,
                },
                Some(caller),
            ) => {
                let call = asks::WorktreeCall {
                    branch,
                    base,
                    brief,
                    name,
                };
                self.tool_start_worktree(pending, &tool, &caller, call);
            }
            (
                Call::Start {
                    directory,
                    name,
                    brief,
                },
                Some(caller),
            ) => {
                let call = asks::StartCall {
                    directory,
                    name,
                    brief,
                };
                self.tool_session_start(pending, &tool, &caller, call);
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

    /// The trust `caller`'s worktree jobs run under: that of an open project whose root is the
    /// session's, as it is now, else the one the session started with.
    fn caller_trust(&self, caller: &Caller) -> Result<Trust, String> {
        let root = self.caller_root(caller).ok_or(
            "your session's project is not trusted; the person trusts it with trust-project",
        )?;
        Ok(Trust {
            root,
            trusted: self.trusted_list(),
        })
    }

    /// The trusted root of `caller`'s project: an open project's whose root is the session's,
    /// looked up now so that the person's later trust or distrust counts, else the one the
    /// session started with (a worktree just made, which no buffer opened).
    fn caller_root(&self, caller: &Caller) -> Option<crate::review::TrustedRoot> {
        let open = caller.root.as_deref().and_then(|root| {
            let id = self
                .editor
                .buffers()
                .filter_map(|(_, buffer)| buffer.project())
                .find(|&id| self.editor.project(id).is_some_and(|p| p.root() == root))?;
            // An open project that is not trusted is not trusted, whatever the session began with.
            Some(crate::review::TrustedRoot::of(&self.editor, id))
        });
        match open {
            Some(now) => now,
            None => self.hosting.trusts.get(&caller.key).cloned(),
        }
    }

    /// The person's input reached the terminal of the session `buffer` holds: its agent may
    /// make as many calls again (ADR 0121, the author's choice: only this resets the guard).
    pub(in crate::app) fn person_typed_in_session(&mut self, buffer: BufferId) {
        if let Some(session) = self.hosting.table.by_buffer(buffer) {
            let id = session.id.clone();
            self.hosting.tools.guard.person_typed(&id);
        }
    }

    /// A report of a worktree job, taken if a tool's call waits for it, else given back.
    pub(in crate::app) fn session_tool_report(&mut self, report: Report) -> Option<Report> {
        // An agent's removal is cleared as the person's is, and its job waits for the end.
        let report = match report {
            Report::Clear { token, path, go }
                if matches!(self.hosting.tools.jobs.get(&token), Some(Job::Removing(_))) =>
            {
                self.tool_clear(token, &path, go);
                return None;
            }
            report => report,
        };
        let token = match &report {
            Report::Prepared { token, .. }
            | Report::Step { token, .. }
            | Report::Created { token, .. }
            | Report::Listed { token, .. }
            | Report::Row { token, .. }
            | Report::Merged { token, .. }
            | Report::ListEnded { token }
            | Report::Removed { token, .. } => *token,
            Report::Branches { .. } | Report::Checked { .. } | Report::Clear { .. } => {
                return Some(report);
            }
        };
        // The agent listing that runs ends with its last report, whether or not a call still
        // waits for it: one cut or given up on is still the agents' to swallow.
        let listing = self
            .hosting
            .tools
            .listing
            .as_ref()
            .is_some_and(|(running, _)| *running == token);
        if listing
            && matches!(
                report,
                Report::ListEnded { .. } | Report::Listed { result: Err(_), .. }
            )
        {
            self.hosting.tools.listing = None;
        }
        let Some(job) = self.hosting.tools.jobs.remove(&token) else {
            return (!listing).then_some(report);
        };
        match job {
            Job::Prepared(preparing) => self.tool_prepared(*preparing, report),
            Job::Creating(creating) => self.tool_creating(token, creating, report),
            Job::Listing(listing) => self.tool_listing(token, listing, report),
            Job::Removing(removing) => self.tool_removed(removing, report),
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
        let dropped: Vec<u64> = self
            .hosting
            .tools
            .jobs
            .iter()
            .filter(|(_, job)| {
                !matches!(job, Job::Creating(_) | Job::Removing(_)) && ours(job.connection())
            })
            .map(|(&token, _)| token)
            .collect();
        for token in dropped {
            self.hosting.tools.jobs.remove(&token);
            self.stop_agent_listing(token);
        }
        // A removal goes on unanswered, and the echo area still says how it went.
        for job in self.hosting.tools.jobs.values_mut() {
            if let Job::Removing(removing) = job
                && ours(removing.connection)
            {
                removing.pending = None;
            }
        }
        let tools = &mut self.hosting.tools;
        tools
            .listings
            .retain(|lister, _| !matches!(lister, Lister::Outside(of) if ours(*of)));
        let closes = tools
            .question
            .as_ref()
            .is_some_and(|question| ours(question.pending.connection));
        if closes {
            self.close_tool_question();
        }
    }

    /// After each event: asks a tool's question once the person is free, takes the answer, and
    /// replies to jobs that ran too long.
    pub(in crate::app) fn sync_session_tools(&mut self) {
        self.ask_about_a_tool();
        self.end_slow_jobs();
    }
}

/// A path as a tool's reply gives it.
fn shown(path: &Path) -> String {
    path.display().to_string()
}
