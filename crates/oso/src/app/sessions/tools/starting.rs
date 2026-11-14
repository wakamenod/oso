//! What a session tool's call runs once it may: on the person's `y`, or at once when the
//! person's configuration lets a session they started start sessions without asking, within
//! `sessions::tools::starts_without_asking`'s rules (ADR 0121 step 6, ADR 0135).
//!
//! Must not depend on: key presses or how a frame is drawn.

use super::super::super::App;
use super::super::hosting::Begin;
use super::asks::{Creating, Plan, Question};
use super::{Caller, Job, shown};
use crate::sessions::record::Origin;
use crate::sessions::tools as rules;
use crate::sessions::worktree::jobs::{Report, Trust};
use crate::sessions::worktree::porcelain::printable;

impl App {
    /// Whether `caller`'s start in a project that is `trusted` or not goes ahead without the
    /// question, by [`rules::starts_without_asking`].
    pub(super) fn starts_without_asking(&self, caller: &Caller, trusted: bool) -> bool {
        let tools = &self.hosting.tools;
        let live = self
            .hosting
            .table
            .all()
            .iter()
            .filter(|session| {
                session.origin == Origin::Unasked
                    && session.runs()
                    && session.parent.as_ref() == Some(&caller.id)
            })
            .count()
            + self.hosting.readying_unasked(&caller.id)
            + tools.unasked_worktrees_being_made(&caller.id);
        rules::starts_without_asking(rules::Unasked {
            allowed: tools.agent_starts.on && !tools.agent_starts_held,
            caller: caller.origin,
            trusted,
            live,
            limit: tools.agent_starts.limit,
        })
    }

    /// The person's `y`, or a start without asking: what the question named runs, as the
    /// person's act or their configuration's.
    pub(super) fn carry_out(&mut self, question: Question) {
        let pending = question.pending;
        match question.plan {
            Plan::Worktree {
                trust,
                order,
                name,
                parent,
                origin,
            } => {
                let token = self.worktree_token();
                let branch = printable(&order.branch, 80);
                // The record of trusted projects as it is at the person's `y`.
                let trust = Trust {
                    trusted: self.trusted_list(),
                    ..trust
                };
                self.worktree_jobs().create(token, trust, order);
                self.editor
                    .message(format!("Making the worktree for {branch}…"));
                self.hosting.tools.jobs.insert(
                    token,
                    Job::Creating(Creating {
                        pending,
                        name,
                        parent,
                        origin,
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
                let key = self.begin_session(Begin {
                    name: creating.name,
                    directory: Some(created.path),
                    brief: Some(created.prompt),
                    parent: Some(creating.parent),
                    origin: creating.origin,
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
}
