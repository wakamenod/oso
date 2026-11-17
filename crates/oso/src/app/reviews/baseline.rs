//! A project's session baseline in the app (ADR 0122): taking one on a thread of its own, as
//! `B` or a hosted session's start asks, and keeping or forgetting it.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here runs git: the thread
//! that takes the baseline does.

use oso_core::project::ProjectId;

use super::state::Baseliner;
use super::{MAX_MESSAGE, ReviewEvent, now, state};
use crate::app::{App, UserEvent};
use crate::commands::review::{RESTRICTED, one_line};
use crate::git::{Id, TrustedRoot};
use crate::review::{Report, Request, Reviewer};

impl App {
    /// Takes a baseline of `project`'s work tree on a thread of its own.
    pub(super) fn take_baseline(&mut self, project: ProjectId) {
        let Some(root) = TrustedRoot::of(&self.editor, project) else {
            return self.editor.message(RESTRICTED);
        };
        let waker = self.waker.clone();
        let serial = self.reviews.serial();
        let started = Reviewer::start(root.clone(), state(), move |report| {
            waker.send(UserEvent::Review(ReviewEvent::Baseline {
                project,
                serial,
                report,
            }))
        });
        match started {
            Ok(reviewer) => {
                reviewer.ask(Request::Baseline);
                self.reviews.add_baseliner(Baseliner {
                    project,
                    serial,
                    root,
                    _reviewer: reviewer,
                });
            }
            Err(e) => self.editor.message(format!("Cannot take a baseline: {e}")),
        }
    }

    pub(super) fn baseline_report(&mut self, project: ProjectId, serial: u64, report: Report) {
        // A report of a thread replaced or dropped since, or taken as the trust was taken
        // back, is not used.
        if !self.reviews.take_baseliner(project, serial) || !self.editor.project_trusted(project) {
            return self.freer.free(report);
        }
        match report {
            Report::Baseline { tree, left_out } => {
                self.keep_baseline(project, tree);
                self.editor.message(if left_out == 0 {
                    "Baseline taken: D reviews what changed since".to_owned()
                } else {
                    format!(
                        "Baseline taken, {left_out} files left out: D reviews what changed since"
                    )
                });
            }
            Report::Failed { why, .. } => {
                let why = one_line(&why, MAX_MESSAGE);
                self.editor
                    .message(format!("Cannot take a baseline: {why}"));
            }
            other => self.freer.free(other),
        }
        self.request_redraw();
    }

    /// `tree` is `project`'s baseline from now on: the session review compares with it.
    fn keep_baseline(&mut self, project: ProjectId, tree: Id) {
        self.reviews.baselines.retain(|(id, _)| *id != project);
        self.reviews.baselines.push((project, tree));
        self.editor.set_baseline_taken(project, Some(now()));
        self.count_session(project);
    }

    /// A hosted session's start thread took `tree` as `project`'s baseline before its
    /// `claude` ran (ADR 0121, ADR 0122): the review takes it as if `B` had, while the
    /// project is still trusted.
    pub(in crate::app) fn session_baseline_taken(&mut self, project: ProjectId, tree: &str) {
        if !self.editor.project_trusted(project) {
            return;
        }
        if let Some(tree) = Id::parse(tree.as_bytes()) {
            self.keep_baseline(project, tree);
        }
    }

    pub(super) fn forget_baseline(&mut self, project: ProjectId) {
        self.reviews.baselines.retain(|(id, _)| *id != project);
        self.editor.set_baseline_taken(project, None);
    }
}
