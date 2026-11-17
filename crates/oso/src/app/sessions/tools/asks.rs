//! The session tools that ask (ADR 0121, step 6): `start_worktree_session` and
//! `session_start`. A job looks first, so that the question names what will happen and a call
//! that cannot happen is refused before anyone is asked. The question opens once the person is
//! free, and takes its answer from the window only, after a quiet moment, as every question an
//! agent causes does. On the person's `y` the worktree is made and the session starts, as the
//! person's act; on `n` or `C-g` the same call from the same session is refused for a minute.
//! When the person's configuration lets the caller start sessions without asking, no question
//! opens and `starting` runs the plan at once (ADR 0135). `session_resume`'s plan is asked and
//! carried out the same way (ADR 0153). The question itself is `question`'s.
//!
//! Must not depend on: key presses or how a frame is drawn.

use std::time::Instant;

use std::path::{Path, PathBuf};

use super::super::super::App;
use super::super::hosting::Begin;
use super::question::details;
use super::{Caller, Job, Pending, shown};
use crate::sessions::identity::Origin;
use crate::sessions::identity::SessionId;
use crate::sessions::tools::{self as rules, Branch, Tool};
use crate::sessions::worktree::create::{Handover, Order};
use crate::sessions::worktree::jobs::{Report, Trust};
use crate::sessions::worktree::prepare::{Prepare, Prepared};

/// What the person's `y` runs.
#[derive(Debug)]
pub(super) enum Plan {
    Worktree {
        trust: Trust,
        order: Order,
        name: Option<String>,
        parent: SessionId,
        origin: Origin,
        /// The directories it also reads, as the job checked them, with their trust.
        reads: Vec<(Trust, PathBuf)>,
    },
    /// A session to start, and for an orchestrator's call the space's repository whose trust
    /// is asked again before it starts.
    Start(Begin, Option<crate::git::TrustedRoot>),
    /// A session started outside oso that moves into a space (ADR 0151).
    Move(Box<super::super::hosting::Moving>),
    /// A stopped session of the caller's space to resume (ADR 0153).
    Resume(super::resume::Resuming),
}

impl Plan {
    /// Starts what it runs as `origin` says: on the person's `y`, or without asking.
    fn started_as(&mut self, now: Origin) {
        match self {
            Self::Worktree { origin, .. } => *origin = now,
            Self::Start(begin, _) => begin.origin = now,
            // It always asks.
            Self::Move(_) => {}
            // Its call's `unasked` says it: a resume keeps the session's origin (ADR 0153).
            Self::Resume(_) => {}
        }
    }
}

/// What a call's question says, what `?` shows, and what the echo area says of a start
/// without asking.
pub(super) struct Words(pub(super) String, pub(super) String, pub(super) String);

/// A call whose job looks before the person is asked.
#[derive(Debug)]
pub(in crate::app) struct Preparing {
    pub(super) pending: Pending,
    caller: Caller,
    target: String,
    trust: Trust,
    then: Then,
    /// The directories the new session also reads, which the job checks too.
    with: Vec<(Trust, PathBuf)>,
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
    /// The directories it also reads, each with its repository's trust (ADR 0136, step 4b).
    pub(in crate::app) with: Vec<(Trust, PathBuf)>,
}

/// `session_start`'s arguments.
#[derive(Debug)]
pub(in crate::app) struct StartCall {
    pub(in crate::app) directory: Option<PathBuf>,
    pub(in crate::app) name: Option<String>,
    pub(in crate::app) brief: Option<String>,
    /// As [`WorktreeCall::with`].
    pub(in crate::app) with: Vec<(Trust, PathBuf)>,
}

/// A worktree being made after the person's `y`.
#[derive(Debug)]
pub(in crate::app) struct Creating {
    pub(super) pending: Pending,
    pub(super) name: Option<String>,
    pub(super) parent: SessionId,
    pub(super) origin: Origin,
    /// The directories its session also reads, with their trust.
    pub(super) reads: Vec<(Trust, PathBuf)>,
    /// What the echo area says once its session starts, for a start without asking.
    pub(super) said: Option<String>,
}

impl App {
    /// The keys of `session-explain` as bound now, which the echo after a start without asking
    /// names (ADR 0143).
    fn explain_key(&self) -> String {
        self.dispatcher.where_is(&self.editor, "session-explain")
    }

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
            with,
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
        self.tool_prepare(pending, tool, caller, branch, (then, prepare, with));
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
            with,
        } = call;
        // An orchestrator no more starts nothing: the space's directory is no repository.
        if caller.scoped() && caller.acting_at.is_none() {
            return pending.answer(Err("your session reaches no repository of its space".into()));
        }
        let directory = directory.unwrap_or_else(|| caller.directory.clone());
        // Kept by the path's parts, so that `/r/` and `/r` are one target.
        let key: PathBuf = directory.components().collect();
        let target = shown(&key);
        // Another directory than the caller's own is looked at by git first, and so is a space's
        // repository an orchestrator names, whose root's identity the job checks on the disk.
        if directory != caller.runs_in || caller.acting_at.is_some() {
            let then = Then::Start { brief, name };
            let prepare = Prepare::Directory(directory);
            return self.tool_prepare(pending, tool, caller, target, (then, prepare, with));
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
        (then, prepare, with): (Then, Prepare, Vec<(Trust, PathBuf)>),
    ) {
        if let Err(why) = self.admit_tool(tool, caller, &target) {
            return pending.answer(Err(why));
        }
        let trust = match self.caller_trust(caller) {
            Ok(trust) => trust,
            Err(why) => return pending.answer(Err(why)),
        };
        let token = self.worktree_token();
        self.worktree_jobs()
            .prepare(token, trust.clone(), prepare, with.clone());
        self.hosting.tools.jobs.insert(
            token,
            Job::Prepared(Box::new(Preparing {
                pending,
                caller: caller.clone(),
                target,
                trust,
                then,
                with,
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
            with,
        } = preparing;
        let Report::Prepared { result, .. } = report else {
            return pending.answer(Err("the call's check ended unexpectedly".into()));
        };
        // Each directory to read was checked with the rest; its start thread checks it again.
        let (prepared, trusted, _) = match result {
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
                let paths: Vec<PathBuf> = with.iter().map(|(_, dir)| dir.clone()).collect();
                let at: (&Path, &str, Branch<'_>) = (&place, &branch, start);
                let words = worktree_words(&caller.name, at, &brief, &paths, &self.explain_key());
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
                        reads: paths,
                    }),
                };
                let plan = Plan::Worktree {
                    trust,
                    order,
                    name,
                    parent: caller.id.clone(),
                    origin: Origin::Answered,
                    reads: with,
                };
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
                    space: App::worker_reads(with),
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
        let details = details(
            &caller.name,
            &label,
            &brief_said(brief.unwrap_or("(no brief)")),
        );
        let lines = brief.map(rules::lines);
        let said =
            rules::start_started(&caller.name, &shown(&directory), lines, &self.explain_key());
        let reads = App::reads_of(begin.space.as_ref());
        let (label, said) = (
            rules::also_reads(label, &reads),
            rules::also_reads(said, &reads),
        );
        let details = rules::also_reads(details, &reads);
        // The new session's first prompt starts with oso's line, never with the agent's text,
        // so that no brief is read as a command of Claude Code's own (`/`, `!`, `#`).
        let also = crate::sessions::worktree::brief::reads_line(&reads)
            .map(|line| format!("{line}\n\n"))
            .unwrap_or_default();
        begin.brief = begin.brief.map(|brief| {
            format!(
                "{also}Session {} started this session through oso, with this brief:\n\n{brief}",
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
    pub(super) fn ask_or_start(
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
        self.queue_question(pending, (caller.id.clone(), target), (label, details), plan);
    }
}

/// What `?` shows for a question: who asks, what, and the whole brief.
/// What a worktree session's question says, what `?` shows, and what the echo area says of
/// its start without asking: `caller` starts one at `place` on `branch` from `start`, with
/// `brief`, also reading `reads`; `explain` is the keys of `session-explain`.
fn worktree_words(
    caller: &str,
    (place, branch, start): (&Path, &str, Branch<'_>),
    brief: &str,
    reads: &[PathBuf],
    explain: &str,
) -> Words {
    let (place, lines) = (shown(place), rules::lines(brief));
    let label = rules::worktree_question(caller, &place, branch, start, lines);
    let label = rules::also_reads(label, reads);
    let said = rules::worktree_started(caller, &place, branch, start, lines, explain);
    let said = rules::also_reads(said, reads);
    let details = details(caller, &label, &brief_said(brief));
    Words(label, details, said)
}

/// What `?` shows of a start's brief.
fn brief_said(brief: &str) -> String {
    format!(
        "The brief, which the new session gets after oso's header and hand-off facts:\n\n{brief}"
    )
}
