//! The agents' session tools in the app (ADR 0121, "The bridge and the channel"; step 6):
//! who calls, what each call may do, and carrying it out with what the sessions and worktree
//! code already has. The rules are `sessions::tools`'; this only applies them.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here touches the disk or
//! starts a process: git runs on the worktree jobs, and a session starts on its start thread,
//! only after the person's `y`, or without asking as the person's configuration lets a session
//! they started (ADR 0135).
//!
//! - A connection proven to be a hosted session's bridge calls as that session. Any other
//!   connection may only read, within the selected window's project (the author's choice), or
//!   ask to move its own session into a space, which asks the person each time (ADR 0151).
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

use super::super::App;
use crate::channel::protocol::sessions::Call;
use crate::channel::{Connection, Reply};
use crate::config::AgentStarts;
use crate::git::TrustedRoot;
use crate::git::printable;
use crate::sessions::hosted::Hosted;
use crate::sessions::identity::Origin;
use crate::sessions::identity::SessionId;
use crate::sessions::tools::{self as rules, Asks, Guard, Tool};
use crate::sessions::worker::SessionKey;
use crate::sessions::worktree::jobs::{Report, Trust};

mod add_repo;
mod asks;
mod moving;
mod question;
mod reads;
mod remove;
mod rename;
mod resume;
mod space;
mod starting;

pub(in crate::app) use add_repo::AddLook;
pub(in crate::app) use question::Question;
use reads::Listing;

/// What a call that would ask is told while another agent's question waits.
const QUESTION_WAITS: &str = "a question an agent caused waits for the person; try again";

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
    /// A `move_to_space` call whose asker is being looked for (ADR 0151).
    looking: Option<moving::Looking>,
    /// When each caller's recent worktree listings started, for their limit.
    listings: HashMap<Lister, Vec<Instant>>,
    /// `*Agent request*`, while it is shown for a question.
    details: Option<BufferId>,
    /// The one agent listing that runs, by its job's token, and what stops it.
    listing: Option<(u64, Arc<AtomicBool>)>,
    /// Whether sessions the person started may start sessions without asking, and how many
    /// each, as `config.toml` said at oso's start (ADR 0135).
    pub(in crate::app) agent_starts: AgentStarts,
    /// `agent-starts-ask` since: every start asks, until `agent-starts-allow` or oso restarts.
    pub(in crate::app) agent_starts_held: bool,
    /// The sessions an agent resumed without asking, by key, with the session that resumed
    /// them: each counts toward that caller's limit while it runs (ADR 0153). Memory only.
    resumed: HashMap<SessionKey, SessionId>,
}

/// A call whose reply waits.
#[derive(Debug)]
pub(in crate::app) struct Pending {
    connection: Connection,
    reply: Sender<Reply>,
    tool: &'static str,
    /// It starts without asking (ADR 0135), which its reply says.
    unasked: bool,
}

impl Tools {
    /// How many worktrees are being made for sessions `parent` starts without asking.
    fn unasked_worktrees_being_made(&self, parent: &SessionId) -> usize {
        self.jobs
            .values()
            .filter(|job| {
                matches!(job, Job::Creating(creating)
                    if creating.origin == Origin::Unasked && &creating.parent == parent)
            })
            .count()
    }
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
    /// Where its calls act: where it runs, or for an orchestrator the repository it names.
    directory: PathBuf,
    /// Where it runs.
    runs_in: PathBuf,
    root: Option<PathBuf>,
    runs: bool,
    /// How it started, for whether it may start sessions without asking (ADR 0135).
    origin: Origin,
    /// Its issue space and its part there, where it is in one (ADR 0136, step 4).
    space: Option<space::InSpace>,
    /// The trust of the space's repository an orchestrator's call names: it acts there, as
    /// from that repository's main checkout.
    acting_at: Option<TrustedRoot>,
}

impl Caller {
    fn of(session: &Hosted) -> Self {
        Self {
            key: session.key,
            id: session.id.clone(),
            name: session.name.clone(),
            directory: session.directory.clone(),
            runs_in: session.directory.clone(),
            root: session.root.clone(),
            runs: session.runs(),
            origin: session.origin,
            space: None,
            acting_at: None,
        }
    }

    /// Whether it is its space's orchestrator, which reaches the space's repositories.
    fn orchestrator(&self) -> bool {
        (self.space.as_ref()).is_some_and(|space| space.role == space::Role::Orchestrator)
    }

    /// Whether it sees its space's sessions only: an orchestrator, current or not.
    fn scoped(&self) -> bool {
        (self.space.as_ref()).is_some_and(|space| space.role != space::Role::Worker)
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
        let caller = (self.hosting.table.by_connection(connection)).map(|s| self.caller_of(s));
        let pending = Pending {
            connection,
            reply,
            tool: tool.name,
            unasked: false,
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
            // Only a session started outside oso moves (ADR 0151).
            (Call::MoveToSpace { space }, None) => self.tool_move_to_space(pending, space),
            (Call::MoveToSpace { .. }, Some(_)) => pending.answer(Err(
                "your session is oso's already; move_to_space is for a session started outside oso"
                    .into(),
            )),
            (call, caller) => self.carry_tool(pending, tool, call, caller),
        }
    }

    /// Carries out `call` of `tool` from `caller`, admitted to the channel.
    fn carry_tool(&mut self, pending: Pending, tool: Tool, call: Call, caller: Option<Caller>) {
        match (call, caller) {
            (Call::List { offset }, caller) => {
                pending.answer(self.session_list(caller.as_ref(), offset))
            }
            (Call::State { session }, caller) => {
                pending.answer(self.session_state(caller.as_ref(), &session))
            }
            (Call::WorktreeList { repo }, caller) => match self.scoped(caller, repo.as_deref()) {
                Ok(caller) => self.tool_worktree_list(pending, caller),
                Err(why) => pending.answer(Err(why)),
            },
            (Call::WorktreeRemove { path }, Some(caller)) => {
                match self.scoped_to_path(caller, &path) {
                    Ok(caller) => self.tool_worktree_remove(pending, &tool, &caller, path),
                    Err(why) => pending.answer(Err(why)),
                }
            }
            (Call::SpaceStatus, caller) => pending.answer(self.space_status(caller.as_ref())),
            (Call::Resume { session }, Some(caller)) => {
                self.tool_session_resume(pending, &tool, &caller, &session);
            }
            (Call::SpaceAddRepo { path }, Some(caller)) => {
                self.tool_space_add_repo(pending, &tool, &caller, path);
            }
            (Call::Rename { session, name }, Some(caller)) => {
                self.tool_session_rename(pending, &tool, &caller, (&session, &name));
            }
            (Call::DraftSet { id, text }, Some(caller)) => {
                pending.answer(self.tool_draft_set(&tool, &caller, &id, &text));
            }
            (
                Call::StartWorktree {
                    branch,
                    base,
                    brief,
                    name,
                    repo,
                    with_dirs,
                },
                Some(caller),
            ) => {
                let (caller, with) = match self.scoped_with(caller, repo.as_deref(), &with_dirs) {
                    Ok(scoped) => scoped,
                    Err(why) => return pending.answer(Err(why)),
                };
                let call = asks::WorktreeCall {
                    branch,
                    base,
                    brief,
                    name,
                    with,
                };
                self.tool_start_worktree(pending, &tool, &caller, call);
            }
            (
                Call::Start {
                    directory,
                    name,
                    brief,
                    repo,
                    with_dirs,
                },
                Some(caller),
            ) => {
                let (caller, with) = match self.scoped_with(caller, repo.as_deref(), &with_dirs) {
                    Ok(scoped) => scoped,
                    Err(why) => return pending.answer(Err(why)),
                };
                let call = asks::StartCall {
                    directory,
                    name,
                    brief,
                    with,
                };
                self.tool_session_start(pending, &tool, &caller, call);
            }
            // `session_tool` carried it out already.
            (Call::MoveToSpace { .. }, _) | (_, None) => {
                pending.answer(Err("this tool is only for a session oso started".into()));
            }
        }
    }

    /// `draft_set` of `text` for request `id` from `caller`, once admitted.
    fn tool_draft_set(&mut self, tool: &Tool, caller: &Caller, id: &str, text: &str) -> Reply {
        self.admit_tool(tool, caller, id)?;
        self.draft_set(caller.key, &caller.runs_in, &caller.name, id, text)
    }

    /// Counts a call of `tool` on `target` from `caller` toward the loop guard, or says why
    /// it is refused: past the guard, within a minute of the person's `n` to the same call, or,
    /// for a tool that asks, while another agent's question waits. A start that may go ahead
    /// without asking is not held back by another's question here; it is if it asks after all.
    fn admit_tool(&mut self, tool: &Tool, caller: &Caller, target: &str) -> Result<(), String> {
        let may_go_unasked = tool.asks == Asks::UnlessAllowed
            && self.agent_starts_allowed()
            && caller.origin.by_the_person();
        if tool.asks != Asks::No && !may_go_unasked && self.agent_question_waits() {
            return Err(QUESTION_WAITS.into());
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
    fn caller_root(&self, caller: &Caller) -> Option<crate::git::TrustedRoot> {
        if let Some(at) = &caller.acting_at {
            return Some(at.clone());
        }
        match self.open_caller_root(caller) {
            Some(now) => now,
            None => self.hosting.trusts.get(&caller.key).cloned(),
        }
    }

    /// The trust of the caller's project as an open buffer of it has it now: `None` when no
    /// buffer has it open, `Some(None)` when it is not trusted.
    fn open_caller_root(&self, caller: &Caller) -> Option<Option<crate::git::TrustedRoot>> {
        caller
            .root
            .as_deref()
            .and_then(|root| self.open_root_trust(root))
    }

    /// The trust of the project at `root` as an open buffer of it has it now: `None` when no
    /// buffer has it open, `Some(None)` when it is not trusted.
    pub(in crate::app) fn open_root_trust(
        &self,
        root: &std::path::Path,
    ) -> Option<Option<crate::git::TrustedRoot>> {
        let id = self
            .editor
            .buffers()
            .filter_map(|(_, buffer)| buffer.project())
            .find(|&id| self.editor.project(id).is_some_and(|p| p.root() == root))?;
        // An open project that is not trusted is not trusted, whatever the session began with.
        Some(crate::git::TrustedRoot::of(&self.editor, id))
    }

    /// Whether the caller's project is trusted now, by an open buffer of it, the session's own
    /// among them: never by the trust the session began with, which the person may have taken
    /// back since (ADR 0135).
    fn caller_trusted_now(&self, caller: &Caller) -> bool {
        if let Some(at) = &caller.acting_at {
            return self.still_trusted(at);
        }
        self.open_caller_root(caller)
            .flatten()
            .is_some_and(|root| !root.revoked())
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
            Report::Branches { .. }
            | Report::Checked { .. }
            | Report::Clear { .. }
            | Report::Tree { .. }
            | Report::TreePulls { .. }
            | Report::Pulls { .. }
            | Report::WorkMerged { .. } => {
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

    /// Whether another agent's question waits for the person, or one of this tool's.
    fn agent_question_waits(&self) -> bool {
        self.hosting.tools.question.is_some() || self.agent_asks()
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
        // An add's look the person has not allowed yet (ADR 0161).
        self.abandon_agent_looks(|look| ours(look.connection()));
        let tools = &mut self.hosting.tools;
        tools
            .listings
            .retain(|lister, _| !matches!(lister, Lister::Outside(of) if ours(*of)));
        // A move's look that waits for nothing any more (ADR 0151).
        tools.looking.take_if(|looking| ours(looking.connection()));
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
        self.sync_moves();
    }
}

/// A path as a tool's reply gives it.
fn shown(path: &Path) -> String {
    path.display().to_string()
}
