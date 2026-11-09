//! Completion past 5000 candidates (ADR 0096): the job a prompt's list leaves for a worker,
//! what the prompt waits for meanwhile, and the report taken in.
//!
//! Must not depend on: `command` or `keymap`, or the host's threads. The host runs the job;
//! this only makes it and takes in what it reports.
//!
//! A newer job supersedes the one before through a generation, which the worker checks as it
//! goes, and whose report is dropped here once it is not the live one. A report is also
//! dropped once its prompt is no longer the innermost, and a match once the list it matched is
//! no longer the prompt's.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::complete::{Build, CompletionJob, CompletionReport, Matching, Report};
use crate::editor::Editor;
use crate::minibuffer::PromptId;

/// What a prompt that completes waits for from the worker.
#[derive(Debug, Default)]
pub(in crate::editor) struct CompletionWork {
    /// The generation of the job that may run, which a worker checks to stop.
    live: Arc<AtomicU64>,
    /// A job for a worker, until the host takes it.
    job: Option<CompletionJob>,
    waiting: Option<Waiting>,
}

/// The job a prompt waits for.
#[derive(Debug)]
struct Waiting {
    generation: u64,
    prompt: PromptId,
    /// The revision of the prompt's answer the job matched.
    revision: u64,
    /// For a build, whose list is not the prompt's yet: whether a segment's directories were
    /// cut.
    build: Option<bool>,
}

impl Editor {
    /// The job a worker is to do, once, for the host.
    pub fn take_completion_job(&mut self) -> Option<CompletionJob> {
        self.completion_work.job.take()
    }

    /// Whether the innermost prompt waits for a worker's result, which its count shows as `…`
    /// and for which `RET` and `TAB` are held (ADR 0096).
    pub fn completion_pending(&self) -> bool {
        self.completion_work
            .waiting
            .as_ref()
            .is_some_and(|waiting| {
                self.prompts
                    .last()
                    .is_some_and(|prompt| prompt.id == waiting.prompt)
            })
    }

    /// Whether the innermost prompt waits for its list to be built, which takes in what is
    /// typed meanwhile once it comes.
    pub(in crate::editor) fn building_completion(&self) -> bool {
        self.completion_pending()
            && self
                .completion_work
                .waiting
                .as_ref()
                .is_some_and(|waiting| waiting.build.is_some())
    }

    /// Leaves `build` for the worker, as the innermost prompt's list, for its answer at
    /// `revision`; `cut` says a segment's directories were cut.
    pub(in crate::editor) fn leave_build(&mut self, build: Build, revision: u64, cut: bool) {
        self.leave(revision, Some(cut), |generation, live| {
            CompletionJob::build(build, generation, live)
        });
    }

    /// Leaves `matching` for the worker, for the innermost prompt's answer at `revision`.
    pub(in crate::editor) fn leave_match(&mut self, matching: Matching, revision: u64) {
        self.leave(revision, None, |generation, live| {
            CompletionJob::matching(matching, generation, live)
        });
    }

    fn leave(
        &mut self,
        revision: u64,
        build: Option<bool>,
        job: impl FnOnce(u64, Arc<AtomicU64>) -> CompletionJob,
    ) {
        let Some(prompt) = self.prompts.last().map(|prompt| prompt.id) else {
            return;
        };
        let work = &mut self.completion_work;
        // The job before stops at its next look.
        let generation = work.live.fetch_add(1, Ordering::Relaxed) + 1;
        work.job = Some(job(generation, Arc::clone(&work.live)));
        work.waiting = Some(Waiting {
            generation,
            prompt,
            revision,
            build,
        });
        if let Some((candidates, _)) = self.completion_mut() {
            candidates.set_pending(true);
        }
    }

    /// Stops the job the innermost prompt waits for, as what it was for is gone.
    pub(in crate::editor) fn stop_completion_job(&mut self) {
        let work = &mut self.completion_work;
        if work.waiting.take().is_some() {
            work.job = None;
            work.live.fetch_add(1, Ordering::Relaxed);
        }
        if let Some((candidates, _)) = self.completion_mut() {
            candidates.set_pending(false);
        }
    }

    /// Takes in what a worker built or matched, unless a newer job has taken its place, its
    /// prompt is no longer the innermost, or the list it matched is no longer the prompt's.
    /// Returns whether what shows changed.
    pub fn completion_reported(&mut self, report: CompletionReport) -> bool {
        let innermost = self.prompts.last().map(|prompt| prompt.id);
        let live = self
            .completion_work
            .waiting
            .as_ref()
            .is_some_and(|waiting| {
                waiting.generation == report.0.generation() && Some(waiting.prompt) == innermost
            });
        if !live {
            return false;
        }
        let Some(waiting) = self.completion_work.waiting.take() else {
            return false;
        };
        match report.0 {
            Report::Built { built, .. } => {
                self.install_files(*built, waiting.build.unwrap_or(false), waiting.revision);
                // What was typed while it was built is matched now.
                self.match_completion();
            }
            Report::Matched { items, matched, .. } => {
                let Some((candidates, _)) = self.completion_mut() else {
                    return false;
                };
                if !Arc::ptr_eq(&items, candidates.items()) {
                    candidates.set_pending(false);
                    return false;
                }
                candidates.apply(matched);
                candidates.set_pending(false);
                self.relayout_prompt();
            }
        }
        true
    }

    /// Drops what the prompt `closed` waited for, and stops its job.
    pub(in crate::editor) fn forget_completion_work(&mut self, closed: PromptId) {
        let waits = self
            .completion_work
            .waiting
            .as_ref()
            .is_some_and(|waiting| waiting.prompt == closed);
        if waits {
            let work = &mut self.completion_work;
            work.waiting = None;
            work.job = None;
            work.live.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests;
