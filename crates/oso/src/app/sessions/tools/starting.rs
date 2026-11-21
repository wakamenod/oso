//! What a session tool's call runs once it may: on the person's `y`, or at once when the
//! person's configuration lets a session they started start sessions without asking, within
//! `sessions::tools::starts_without_asking`'s rules (ADR 0121 step 6, ADR 0135, ADR 0153,
//! ADR 0161); whether the configuration allows that now; and the reply once a session a call
//! asked for started.
//!
//! Must not depend on: key presses or how a frame is drawn.

use serde_json::json;

use super::super::super::App;
use super::super::hosting::Begin;
use super::asks::{Creating, Plan};
use super::{Caller, Job, Pending, shown};
use crate::git::printable;
use crate::sessions::explain::{self, Unasked};
use crate::sessions::identity::{Origin, SessionId};
use crate::sessions::tools as rules;
use crate::sessions::worker::SessionKey;
use crate::sessions::worktree::jobs::{Report, Trust};

impl App {
    /// Whether `caller`'s call of `tool`, a start in a project that is `trusted` now or not,
    /// goes ahead without the question, by [`rules::starts_without_asking`].
    pub(super) fn starts_without_asking(&self, caller: &Caller, tool: &str, trusted: bool) -> bool {
        let tools = &self.hosting.tools;
        let tool = rules::tool(tool);
        // An add starts no session, so it counts toward no limit (ADR 0161, Q4).
        let (live_unasked, limit) = if tool.is_some_and(|tool| tool.starts) {
            (self.unasked_children(&caller.id), tools.agent_starts.limit)
        } else {
            (0, rules::StartLimit::Unlimited)
        };
        rules::starts_without_asking(rules::Unasked {
            tool: tool.map_or(rules::Asks::Each, |tool| tool.asks),
            allowed: self.agent_starts_allowed(),
            caller: caller.origin,
            trusted,
            live_unasked,
            limit,
        })
    }

    /// How many sessions `parent` started or resumed without asking that run, are being got
    /// ready, or wait for their worktree (ADR 0135, ADR 0153).
    fn unasked_children(&self, parent: &SessionId) -> usize {
        let resumed = &self.hosting.tools.resumed;
        let running = self.hosting.table().all().iter().filter(|session| {
            let started =
                session.origin == Origin::Unasked && session.parent.as_ref() == Some(parent);
            session.runs() && (started || resumed.get(&session.key) == Some(parent))
        });
        running.count()
            + self.hosting.readying_unasked(parent)
            + self.hosting.tools.unasked_worktrees_being_made(parent)
    }

    /// The person's `y`, or a start without asking: what the question named runs, as the
    /// person's act or their configuration's. `said`, for a start without asking, is said once
    /// the session's start is under way, so that the echo area ends on it.
    pub(super) fn carry_out(&mut self, pending: Pending, plan: Plan, said: Option<String>) {
        match plan {
            Plan::Worktree {
                trust,
                order,
                name,
                parent,
                origin,
                reads,
            } => {
                // As the reads stand at the person's `y`.
                let reads = self.reads_now(reads);
                let token = self.worktree_token();
                let branch = printable(&order.branch, 80);
                // The record of trusted projects as it is at the person's `y`.
                let trust = Trust {
                    trusted: self.trusted_list(),
                    ..trust
                };
                self.worktree_jobs().create(token, trust, order);
                let by = if said.is_some() { " for an agent" } else { "" };
                self.editor
                    .message(format!("Making the worktree for {branch}{by}…"));
                self.hosting.tools.jobs.insert(
                    token,
                    Job::Creating(Creating {
                        pending,
                        name,
                        parent,
                        origin,
                        reads,
                        said,
                    }),
                );
            }
            // A space's repository, unless its trust was taken back since the question.
            Plan::Start(_, Some(root)) if !self.still_trusted(&root) => {
                pending.answer(Err("the repository is no longer trusted".into()));
            }
            Plan::Move(moving) => self.start_move(pending, *moving),
            Plan::Resume(resuming) => self.carry_out_resume(pending, resuming, said),
            Plan::AddRepo(adding) => self.carry_out_add(pending, *adding, said),
            Plan::Start(mut begin, _) => {
                // As the directories to read stand at the person's `y`.
                begin.space = self.space_now(begin.space.take());
                match self.begin_session(begin) {
                    Some(key) => {
                        self.hosting.tools.starts.insert(key, pending);
                        if let Some(said) = said {
                            self.editor.message(format!("Agent: {said}"));
                        }
                    }
                    None => pending.answer(Err("the session did not start".into())),
                }
            }
        }
    }

    /// A worktree job made, or did not make, the worktree the person allowed.
    pub(super) fn tool_creating(&mut self, token: u64, creating: Creating, report: Report) {
        if matches!(report, Report::Created { .. }) {
            // The echo area says how it went.
            self.request_redraw();
        }
        match report {
            Report::Step { .. } => {
                self.hosting
                    .tools
                    .jobs
                    .insert(token, Job::Creating(creating));
            }
            Report::Created {
                result: Ok((created, trusted)),
                ..
            } => {
                self.editor.message(format!(
                    "Worktree {} made on {}",
                    printable(&shown(&created.path), 200),
                    printable(&created.branch, 80)
                ));
                // Held as the person's worktree would be (ADR 0136).
                let asker = Some(&creating.parent);
                self.hold_made_worktree(asker, &created.path, &created.main);
                // As the reads stand now, the worktree made.
                let reads = self.reads_now(creating.reads);
                let key = self.begin_session(Begin {
                    name: creating.name,
                    directory: Some(created.path),
                    brief: Some(created.prompt),
                    parent: Some(creating.parent),
                    origin: creating.origin,
                    check_shared: false,
                    trusted: Some(trusted),
                    space: App::worker_reads(reads),
                    behind: false,
                });
                match key {
                    Some(key) => {
                        self.hosting.tools.starts.insert(key, creating.pending);
                        if let Some(said) = creating.said {
                            self.editor.message(format!("Agent: {said}"));
                        }
                    }
                    None => creating.pending.answer(Err(
                        "the worktree was made; its session did not start".into(),
                    )),
                }
            }
            Report::Created {
                result: Err(why), ..
            } => {
                self.editor.message(why.clone());
                creating.pending.answer(Err(why));
            }
            _ => creating
                .pending
                .answer(Err("making the worktree ended unexpectedly".into())),
        }
    }

    /// The explanation `explained` of session `key`, with who started it and its brief when an
    /// agent started it without asking; `None` for any other session (ADR 0135, as built).
    pub(in crate::app) fn unasked_explained(
        &self,
        key: SessionKey,
        explained: &str,
    ) -> Option<String> {
        let table = self.hosting.table();
        let session = table.get(key)?;
        if session.origin != Origin::Unasked {
            return None;
        }
        let by = session
            .parent
            .as_ref()
            .and_then(|parent| table.all().iter().find(|s| &s.id == parent))
            .map_or_else(
                || "a session that is gone".to_owned(),
                |parent| format!("session {}", rules::quoted(&parent.name)),
            );
        let kept = session
            .unasked_brief
            .as_deref()
            .map_or(Unasked::NotKept, Unasked::Brief);
        Some(explain::unasked(&by, kept, explained))
    }

    /// Whether the configuration and the commands let sessions the person started start
    /// sessions without asking now (ADR 0135).
    pub(super) fn agent_starts_allowed(&self) -> bool {
        let tools = &self.hosting.tools;
        tools.agent_starts.on && !tools.agent_starts_held
    }

    /// `agent-starts-ask` when `held`, `agent-starts-allow` when not: whether sessions the person
    /// started may start sessions without asking, as the configuration said at oso's start
    /// (ADR 0135).
    pub(in crate::app) fn hold_agent_starts(&mut self, held: bool) {
        let tools = &mut self.hosting.tools;
        tools.agent_starts_held = held;
        let said = match (held, tools.agent_starts.on) {
            (true, _) => "Sessions started by agents ask you first, until oso restarts",
            (false, true) => {
                "Sessions started by agents start without asking again, as the configuration says"
            }
            (false, false) => {
                "Sessions started by agents ask you first: the configuration does not set \
                 [sessions] agents_start_without_asking"
            }
        };
        self.editor.message(said);
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
            (Ok(()), Some(session)) if pending.unasked => Ok(json!({
                "session": { "id": session.id.as_str(), "name": session.name },
                "directory": session.directory.display().to_string(),
                "asked": false,
                "note": "started without asking, as the person's config.toml allows",
            })),
            (Ok(()), Some(session)) => Ok(json!({
                "session": { "id": session.id.as_str(), "name": session.name },
                "directory": session.directory.display().to_string(),
                "asked": true,
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
}
