//! The session tools that ask (ADR 0121, step 6): `start_worktree_session` and
//! `session_start`. A job looks first, so that the question names what will happen and a call
//! that cannot happen is refused before anyone is asked. The question opens once the person is
//! free, and takes its answer from the window only, after a quiet moment, as every question an
//! agent causes does. On the person's `y` the worktree is made and the session starts, as the
//! person's act; on `n` or `C-g` the same call from the same session is refused for a minute.
//! When the person's configuration lets the caller start sessions without asking, no question
//! opens and `starting` runs the plan at once (ADR 0135).
//!
//! Must not depend on: key presses or how a frame is drawn.

use std::time::Instant;

use std::path::PathBuf;

use oso_core::buffer::{Buffer, Mode};
use oso_core::minibuffer::PromptId;

use super::super::super::App;
use super::super::hosting::Begin;
use super::{Caller, Job, Pending, shown};
use crate::sessions::identity::Origin;
use crate::sessions::identity::SessionId;
use crate::sessions::tools::{self as rules, Branch, Tool};
use crate::sessions::worktree::create::{Handover, Order};
use crate::sessions::worktree::jobs::{Report, Trust};
use crate::sessions::worktree::prepare::{Prepare, Prepared};

/// The name of the buffer `?` shows.
const DETAILS: &str = "*Agent request*";
/// What the agent is told when the person says no.
const REFUSED: &str =
    "the person refused it; that is their decision: do not reach the same effect another way";

/// A tool's question, before and while it is asked.
#[derive(Debug)]
pub(in crate::app) struct Question {
    pub(super) pending: Pending,
    caller: SessionId,
    /// What the refusal is kept for: the branch, or the directory.
    target: String,
    label: String,
    /// What `?` shows.
    details: String,
    pub(super) plan: Plan,
    asked: Option<PromptId>,
}

/// What the person's `y` runs.
#[derive(Debug)]
pub(super) enum Plan {
    Worktree {
        trust: Trust,
        order: Order,
        name: Option<String>,
        parent: SessionId,
        origin: Origin,
    },
    /// A session to start, and for an orchestrator's call the space's repository whose trust
    /// is asked again before it starts.
    Start(Begin, Option<crate::review::TrustedRoot>),
}

impl Plan {
    /// Starts what it runs as `origin` says: on the person's `y`, or without asking.
    fn started_as(&mut self, now: Origin) {
        match self {
            Self::Worktree { origin, .. } => *origin = now,
            Self::Start(begin, _) => begin.origin = now,
        }
    }
}

/// What a call's question says, what `?` shows, and what the echo area says of a start
/// without asking.
struct Words(String, String, String);

/// A call whose job looks before the person is asked.
#[derive(Debug)]
pub(in crate::app) struct Preparing {
    pub(super) pending: Pending,
    caller: Caller,
    target: String,
    trust: Trust,
    then: Then,
}

/// What the call does once looked at.
#[derive(Debug)]
enum Then {
    Worktree {
        brief: String,
        name: Option<String>,
        base: Option<String>,
    },
    Start {
        brief: Option<String>,
        name: Option<String>,
    },
}

/// `start_worktree_session`'s arguments.
#[derive(Debug)]
pub(in crate::app) struct WorktreeCall {
    pub(in crate::app) branch: String,
    pub(in crate::app) base: Option<String>,
    pub(in crate::app) brief: String,
    pub(in crate::app) name: Option<String>,
}

/// `session_start`'s arguments.
#[derive(Debug)]
pub(in crate::app) struct StartCall {
    pub(in crate::app) directory: Option<PathBuf>,
    pub(in crate::app) name: Option<String>,
    pub(in crate::app) brief: Option<String>,
}

/// A worktree being made after the person's `y`.
#[derive(Debug)]
pub(in crate::app) struct Creating {
    pub(super) pending: Pending,
    pub(super) name: Option<String>,
    pub(super) parent: SessionId,
    pub(super) origin: Origin,
    /// What the echo area says once its session starts, for a start without asking.
    pub(super) said: Option<String>,
}

impl App {
    /// `start_worktree_session` from `caller`: a job resolves the branch and its start before
    /// the person is asked.
    pub(super) fn tool_start_worktree(
        &mut self,
        pending: Pending,
        tool: &Tool,
        caller: &Caller,
        call: WorktreeCall,
    ) {
        let WorktreeCall {
            branch,
            base,
            brief,
            name,
        } = call;
        let then = Then::Worktree {
            brief,
            name,
            base: base.clone(),
        };
        let prepare = Prepare::Worktree {
            branch: branch.clone(),
            base,
            caller: caller.directory.clone(),
        };
        // Kept by the name typed until the job resolves it; then by the branch git names.
        self.tool_prepare(pending, tool, caller, branch, (then, prepare));
    }

    /// `session_start` from `caller`: in its own directory at once, elsewhere once a job has
    /// looked that the directory is its repository's.
    pub(super) fn tool_session_start(
        &mut self,
        pending: Pending,
        tool: &Tool,
        caller: &Caller,
        call: StartCall,
    ) {
        let StartCall {
            directory,
            name,
            brief,
        } = call;
        let directory = directory.unwrap_or_else(|| caller.directory.clone());
        // Kept by the path's parts, so that `/r/` and `/r` are one target.
        let key: PathBuf = directory.components().collect();
        let target = shown(&key);
        // Another directory than the caller's own is looked at by git first, and so is a space's
        // repository an orchestrator names, whose root's identity the job checks on the disk.
        if directory != caller.runs_in || caller.acting_at.is_some() {
            let then = Then::Start { brief, name };
            let prepare = Prepare::Directory(directory);
            return self.tool_prepare(pending, tool, caller, target, (then, prepare));
        }
        if let Err(why) = self.admit_tool(tool, caller, &target) {
            return pending.answer(Err(why));
        }
        // The caller's own directory: no git, so a restricted project may host it too.
        let begin = Begin {
            name,
            directory: Some(caller.directory.clone()),
            brief,
            parent: Some(caller.id.clone()),
            origin: Origin::Answered,
            check_shared: false,
            trusted: self.caller_root(caller),
            space: None,
        };
        // Without asking only in a project an open buffer has trusted now: the trust the session
        // began with may have been taken back since (ADR 0135, Q4).
        let trusted = self.caller_trusted_now(caller);
        self.queue_start_question(pending, caller, (target, trusted), begin);
    }

    /// Admits `pending`'s call on `target`, and starts the job that looks at what `prepare`
    /// names before the person is asked.
    fn tool_prepare(
        &mut self,
        pending: Pending,
        tool: &Tool,
        caller: &Caller,
        target: String,
        (then, prepare): (Then, Prepare),
    ) {
        if let Err(why) = self.admit_tool(tool, caller, &target) {
            return pending.answer(Err(why));
        }
        let trust = match self.caller_trust(caller) {
            Ok(trust) => trust,
            Err(why) => return pending.answer(Err(why)),
        };
        let token = self.worktree_token();
        self.worktree_jobs().prepare(token, trust.clone(), prepare);
        self.hosting.tools.jobs.insert(
            token,
            Job::Prepared(Box::new(Preparing {
                pending,
                caller: caller.clone(),
                target,
                trust,
                then,
            })),
        );
    }

    /// The job looked at what `preparing`'s call would do: its question waits for the person.
    pub(super) fn tool_prepared(&mut self, preparing: Preparing, report: Report) {
        let Preparing {
            pending,
            caller,
            target,
            trust,
            then,
        } = preparing;
        let Report::Prepared { result, .. } = report else {
            return pending.answer(Err("the call's check ended unexpectedly".into()));
        };
        let (prepared, trusted) = match result {
            Ok(found) => found,
            Err(why) => return pending.answer(Err(why)),
        };
        match (prepared, then) {
            (
                Prepared::Worktree {
                    place,
                    branch,
                    start,
                },
                Then::Worktree { brief, name, base },
            ) => {
                // The person's `n` is kept by the branch git resolved the name to, so that
                // another spelling of the same branch is refused as well.
                let tool = pending.tool;
                if branch != target
                    && self.hosting.tools.guard.was_refused(
                        &caller.id,
                        tool,
                        &branch,
                        Instant::now(),
                    )
                {
                    return pending.answer(Err(rules::Refusal::Person.why().into()));
                }
                let target = branch.clone();
                let from = start.as_ref().map(|(_, name)| name.as_str());
                let start = from.map_or(Branch::Existing, |from| Branch::New { from });
                let (place, lines) = (shown(&place), rules::lines(&brief));
                let label = rules::worktree_question(&caller.name, &place, &branch, start, lines);
                let said = rules::worktree_started(&caller.name, &place, &branch, start, lines);
                let details = details(&caller.name, &label, &brief);
                // The job that makes it resolves the start again, as the question named it: the
                // base given, else the caller's `HEAD`.
                let order = Order {
                    branch,
                    base,
                    start_at: Some(caller.directory.clone()),
                    brief,
                    from: Some(Handover {
                        name: caller.runs.then(|| caller.name.clone()),
                        id: caller.id.as_str().to_owned(),
                        // Where it runs: its own uncommitted files, not the repository's it names.
                        directory: caller.runs_in.clone(),
                        space: self.space_orchestrated_by(&caller),
                    }),
                };
                let plan = Plan::Worktree {
                    trust,
                    order,
                    name,
                    parent: caller.id.clone(),
                    origin: Origin::Answered,
                };
                let words = Words(label, details, said);
                self.ask_or_start(pending, &caller, (target, true), words, plan);
            }
            (Prepared::Directory(directory), Then::Start { brief, name }) => {
                let begin = Begin {
                    name,
                    directory: Some(directory),
                    brief,
                    parent: Some(caller.id.clone()),
                    origin: Origin::Answered,
                    check_shared: false,
                    trusted,
                    space: None,
                };
                // Its job, as a worktree's, looked at trust against the record as it is now.
                let now = (target, begin.trusted.is_some());
                self.queue_start_question(pending, &caller, now, begin);
            }
            _ => pending.answer(Err("the call's check found something else".into())),
        }
    }

    fn queue_start_question(
        &mut self,
        pending: Pending,
        caller: &Caller,
        (target, trusted): (String, bool),
        begin: Begin,
    ) {
        let mut begin = begin;
        let directory = begin.directory.clone().unwrap_or_default();
        // Another session than the caller's own runs there.
        let shared = self
            .hosting
            .table
            .all()
            .iter()
            .any(|s| s.key != caller.key && s.runs() && s.directory == directory);
        let brief = begin.brief.as_deref();
        let label = rules::start_question(
            &caller.name,
            &shown(&directory),
            brief.map(rules::lines),
            shared,
        );
        let details = details(&caller.name, &label, brief.unwrap_or("(no brief)"));
        let said = rules::start_started(&caller.name, &shown(&directory), brief.map(rules::lines));
        // The new session's first prompt starts with oso's line, never with the agent's text,
        // so that no brief is read as a command of Claude Code's own (`/`, `!`, `#`).
        begin.brief = begin.brief.map(|brief| {
            format!(
                "Session {} started this session through oso, with this brief:\n\n{brief}",
                rules::quoted(&caller.name)
            )
        });
        let words = Words(label, details, said);
        self.ask_or_start(
            pending,
            caller,
            (target, trusted),
            words,
            Plan::Start(begin, caller.acting_at.clone()),
        );
    }

    /// Starts `plan`, `caller`'s call on `target`, at once when the person's configuration lets
    /// it start without asking (ADR 0135), else asks the person; `trusted` when the project it
    /// starts in is trusted now.
    fn ask_or_start(
        &mut self,
        mut pending: Pending,
        caller: &Caller,
        (target, trusted): (String, bool),
        words: Words,
        mut plan: Plan,
    ) {
        let Words(label, details, said) = words;
        if self.starts_without_asking(caller, pending.tool, trusted) {
            plan.started_as(Origin::Unasked);
            pending.unasked = true;
            self.agent_log(&format!("{}: starting without asking", pending.tool));
            return self.carry_out(pending, plan, Some(said));
        }
        if self.agent_question_waits() {
            return pending.answer(Err(super::QUESTION_WAITS.into()));
        }
        // A `y` here lets the new session start sessions without asking in turn (Q3): the
        // question says when the one asking began without the person.
        let label = if caller.origin.by_the_person() {
            label
        } else {
            format!("{label} It started without asking itself.")
        };
        self.hosting.tools.question = Some(super::Question {
            pending,
            caller: caller.id.clone(),
            target,
            label,
            details,
            plan,
            asked: None,
        });
        self.ask_about_a_tool();
    }

    /// Asks the waiting tool's question once the person is free, and takes the answer once it
    /// has closed.
    pub(super) fn ask_about_a_tool(&mut self) {
        let Some(question) = &self.hosting.tools.question else {
            return;
        };
        match question.asked {
            None => {
                if !self.agent_question_may_open() {
                    return;
                }
                let label = question.label.clone();
                match self.editor.ask_for_agent(
                    label,
                    "agent-channel-allow",
                    crate::app::agent::AGENT_VERBS,
                ) {
                    Ok(id) => {
                        if let Some(question) = &mut self.hosting.tools.question {
                            question.asked = Some(id);
                        }
                        self.note_agent_question_opened();
                        self.request_redraw();
                    }
                    Err(e) => {
                        if let Some(question) = self.hosting.tools.question.take() {
                            question.pending.answer(Err(e.to_string()));
                        }
                    }
                }
            }
            Some(id) => {
                if self.editor.prompts().iter().any(|prompt| prompt.id() == id) {
                    return;
                }
                let Some(question) = self.hosting.tools.question.take() else {
                    return;
                };
                self.hide_question_details();
                if self.editor.take_agent_allowed() {
                    self.agent_log(&format!("{}: allowed", question.pending.tool));
                    self.carry_out(question.pending, question.plan, None);
                } else {
                    let tool = question.pending.tool;
                    self.hosting.tools.guard.refused(
                        &question.caller,
                        tool,
                        &question.target,
                        Instant::now(),
                    );
                    self.agent_log(&format!("{tool}: refused"));
                    question.pending.answer(Err(REFUSED.into()));
                }
            }
        }
    }

    /// The open question of a session tool, if one is asked.
    pub(in crate::app) fn session_tool_question(&self) -> Option<PromptId> {
        self.hosting.tools.question.as_ref().and_then(|q| q.asked)
    }

    /// The question's connection went: it closes, and nothing it asked for runs.
    pub(super) fn close_tool_question(&mut self) {
        let Some(question) = self.hosting.tools.question.take() else {
            return;
        };
        self.hide_question_details();
        if let Some(id) = question.asked {
            self.orphan_agent_question(id);
        }
    }

    /// `?` in the tool's open question: `*Agent request*` shows what it would run.
    pub(in crate::app) fn show_question_details(&mut self) {
        let Some(question) = self
            .hosting
            .tools
            .question
            .as_ref()
            .filter(|question| question.asked.is_some())
        else {
            return self
                .editor
                .message("This question has nothing more to show");
        };
        let text = question.details.clone();
        let kept = self.hosting.tools.details;
        let id = match kept.filter(|&id| self.editor.buffer(id).is_some_and(Buffer::is_internal)) {
            Some(id) => id,
            None => self.editor.add_internal_buffer(DETAILS, Mode::Fundamental),
        };
        self.hosting.tools.details = Some(id);
        // The agent's own request: no agent reads it back, as none reads `*Brief*`.
        if let Some(buffer) = self.editor.buffer_mut(id) {
            buffer.withhold_from_agents();
        }
        self.editor.set_internal_text(id, &text);
        self.editor.show(id);
        self.request_redraw();
    }

    fn hide_question_details(&mut self) {
        if let Some(id) = self.hosting.tools.details.take()
            && self.editor.buffer(id).is_some()
        {
            self.editor.kill_buffer(id);
        }
    }
}

/// What `?` shows for a question: who asks, what, and the whole brief.
fn details(caller: &str, label: &str, brief: &str) -> String {
    format!(
        "Session {} asks:\n\n  {label}\n\ny allows it, n or C-g refuses it.\n\nThe brief, which the new session gets after oso's header and hand-off facts:\n\n{brief}\n",
        rules::quoted(caller)
    )
}
