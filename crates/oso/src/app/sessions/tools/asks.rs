//! The session tools that ask (ADR 0121, step 6): `start_worktree_session` and
//! `session_start`. A job looks first, so that the question names what will happen and a call
//! that cannot happen is refused before anyone is asked. The question opens once the person is
//! free, and takes its answer from the window only, after a quiet moment, as every question an
//! agent causes does. On the person's `y` the worktree is made and the session starts, as the
//! person's act; on `n` or `C-g` the same call from the same session is refused for a minute.
//!
//! Must not depend on: key presses or how a frame is drawn.

use std::time::Instant;

use oso_core::buffer::{Buffer, Mode};
use oso_core::minibuffer::PromptId;

use super::super::super::App;
use super::super::hosting::Begin;
use super::{Caller, Job, Pending, shown};
use crate::channel::protocol::sessions::Call;
use crate::sessions::identity::SessionId;
use crate::sessions::tools::{self as rules, Branch, Tool};
use crate::sessions::worktree::create::{Handover, Order};
use crate::sessions::worktree::jobs::{Report, Trust};
use crate::sessions::worktree::porcelain::printable;
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
    plan: Plan,
    asked: Option<PromptId>,
}

/// What the person's `y` runs.
#[derive(Debug)]
enum Plan {
    Worktree {
        trust: Trust,
        order: Order,
        name: Option<String>,
        parent: SessionId,
    },
    Start(Begin),
}

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
    },
    Start {
        brief: Option<String>,
        name: Option<String>,
    },
}

/// A worktree being made after the person's `y`.
#[derive(Debug)]
pub(in crate::app) struct Creating {
    pub(super) pending: Pending,
    name: Option<String>,
    parent: SessionId,
}

impl App {
    /// `start_worktree_session` or `session_start` from `caller`.
    pub(super) fn tool_start(
        &mut self,
        pending: Pending,
        tool: &Tool,
        caller: &Caller,
        call: Call,
    ) {
        let (target, then, prepare) = match call {
            Call::StartWorktree {
                branch,
                base,
                brief,
                name,
            } => (
                branch.clone(),
                Then::Worktree { brief, name },
                Some(Prepare::Worktree {
                    branch,
                    base,
                    caller: caller.directory.clone(),
                }),
            ),
            Call::Start {
                directory,
                name,
                brief,
            } => {
                let directory = directory.unwrap_or_else(|| caller.directory.clone());
                // Another directory than the caller's own is looked at by git first.
                let prepare =
                    (directory != caller.directory).then(|| Prepare::Directory(directory.clone()));
                (shown(&directory), Then::Start { brief, name }, prepare)
            }
            _ => return pending.answer(Err("not a tool that starts a session".into())),
        };
        if let Err(why) = self.admit_tool(tool, caller, &target) {
            return pending.answer(Err(why));
        }
        let Some(prepare) = prepare else {
            // The caller's own directory: no git, so a restricted project may host it too.
            let Then::Start { brief, name } = then else {
                return;
            };
            let trusted = self.hosting.trusts.get(&caller.key).cloned();
            let begin = Begin {
                name,
                directory: Some(caller.directory.clone()),
                brief,
                parent: Some(caller.id.clone()),
                check_shared: false,
                trusted,
            };
            return self.queue_start_question(pending, caller, target, begin);
        };
        let trust = match self.caller_trust(caller) {
            Ok(trust) => trust,
            Err(why) => return pending.answer(Err(why)),
        };
        let token = self.worktree_token();
        self.worktree_jobs().prepare(token, trust.clone(), prepare);
        self.hosting.tools.jobs.insert(
            token,
            Job::Prepared(Preparing {
                pending,
                caller: caller.clone(),
                target,
                trust,
                then,
            }),
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
                Then::Worktree { brief, name },
            ) => {
                let from = start.as_ref().map(|(_, shown)| shown.as_str());
                let label = rules::worktree_question(
                    &caller.name,
                    &shown(&place),
                    &branch,
                    from.map_or(Branch::Existing, |from| Branch::New { from }),
                    rules::lines(&brief),
                );
                let details = details(&caller.name, &label, &brief);
                let order = Order {
                    branch,
                    base: start.map(|(id, _)| id),
                    brief,
                    from: Some(Handover {
                        name: caller.runs.then(|| caller.name.clone()),
                        id: caller.id.as_str().to_owned(),
                        directory: caller.directory.clone(),
                    }),
                };
                let plan = Plan::Worktree {
                    trust,
                    order,
                    name,
                    parent: caller.id.clone(),
                };
                self.queue_question(pending, &caller, target, (label, details), plan);
            }
            (Prepared::Directory(directory), Then::Start { brief, name }) => {
                let begin = Begin {
                    name,
                    directory: Some(directory),
                    brief,
                    parent: Some(caller.id.clone()),
                    check_shared: false,
                    trusted,
                };
                self.queue_start_question(pending, &caller, target, begin);
            }
            _ => pending.answer(Err("the call's check found something else".into())),
        }
    }

    fn queue_start_question(
        &mut self,
        pending: Pending,
        caller: &Caller,
        target: String,
        begin: Begin,
    ) {
        let directory = begin.directory.clone().unwrap_or_default();
        let shared = self.hosting.table.runs_in(&directory);
        let brief = begin.brief.as_deref();
        let label = rules::start_question(
            &caller.name,
            &shown(&directory),
            brief.map(rules::lines),
            shared,
        );
        let details = details(&caller.name, &label, brief.unwrap_or("(no brief)"));
        self.queue_question(
            pending,
            caller,
            target,
            (label, details),
            Plan::Start(begin),
        );
    }

    fn queue_question(
        &mut self,
        pending: Pending,
        caller: &Caller,
        target: String,
        (label, details): (String, String),
        plan: Plan,
    ) {
        if self.hosting.tools.question.is_some() {
            return pending.answer(Err(
                "a question an agent caused waits for the person; try again".into(),
            ));
        }
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
                    self.carry_out(question);
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

    /// The person's `y`: what the question named runs, as the person's act.
    fn carry_out(&mut self, question: Question) {
        let pending = question.pending;
        match question.plan {
            Plan::Worktree {
                trust,
                order,
                name,
                parent,
            } => {
                let token = self.worktree_token();
                let branch = printable(&order.branch, 80);
                self.worktree_jobs().create(token, trust, order);
                self.editor
                    .message(format!("Making the worktree for {branch}…"));
                self.hosting.tools.jobs.insert(
                    token,
                    Job::Creating(Creating {
                        pending,
                        name,
                        parent,
                    }),
                );
            }
            Plan::Start(begin) => match self.begin_session(begin) {
                Some(key) => {
                    self.hosting.tools.starts.insert(key, pending);
                }
                None => pending.answer(Err("the session did not start".into())),
            },
        }
    }

    /// A worktree job made, or did not make, the worktree the person allowed.
    pub(super) fn tool_creating(&mut self, token: u64, creating: Creating, report: Report) {
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
                let key = self.begin_session(Begin {
                    name: creating.name,
                    directory: Some(created.path),
                    brief: Some(created.prompt),
                    parent: Some(creating.parent),
                    check_shared: false,
                    trusted: Some(trusted),
                });
                match key {
                    Some(key) => {
                        self.hosting.tools.starts.insert(key, creating.pending);
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
