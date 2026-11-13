//! Completion past 5000 candidates (ADR 0096): the jobs a prompt's list leaves for a worker,
//! what the prompt waits for meanwhile, and the report taken in.
//!
//! Must not depend on: `command` or `keymap`, or the host's threads. The host runs the jobs;
//! this only makes them and takes in what they report.
//!
//! A newer job supersedes the one before through a generation, which the worker checks as it
//! goes, and whose report is dropped here once it is not the live one. A report is also
//! dropped once its prompt is not the innermost, and the prompt is then matched again when it
//! is, and a match once the list it matched is no longer the prompt's. What the prompt no
//! longer holds of a list past 5000, a report dropped and a job dropped before the host took
//! it are freed by a job of their own, on the worker.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::complete::{
    Build, Candidates, CompletionJob, CompletionReport, Matching, Merge, Report, WORKER_PAST,
};
use crate::editor::Editor;
use crate::minibuffer::{Prompt, PromptId};

/// What a prompt that completes waits for from the worker.
#[derive(Debug, Default)]
pub(in crate::editor) struct CompletionWork {
    /// The generation of the job that may run, which a worker checks to stop.
    live: Arc<AtomicU64>,
    /// Jobs for a worker, until the host takes them, oldest first.
    jobs: VecDeque<CompletionJob>,
    waiting: Option<Waiting>,
}

impl CompletionWork {
    /// Drops the jobs the host has not taken, but those that only free. They go to one job
    /// that frees them on the worker, as the last copy of a list or of a dictionary that was
    /// replaced may be theirs, and dropping it here would hold the UI thread.
    fn drop_jobs(&mut self) {
        let mut dropped = Vec::new();
        for job in std::mem::take(&mut self.jobs) {
            if job.frees_only() {
                self.jobs.push_back(job);
            } else {
                dropped.push(job);
            }
        }
        if !dropped.is_empty() {
            self.jobs.push_back(CompletionJob::free(Box::new(dropped)));
        }
    }
}

/// The job a prompt waits for.
#[derive(Debug)]
struct Waiting {
    generation: u64,
    prompt: PromptId,
    /// The revision of the prompt's answer the job matched.
    revision: u64,
    kind: Kind,
}

/// What a job a prompt waits for makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// The matches of a newer input, over the prompt's list.
    Match,
    /// A list that is not the prompt's yet, and whether a segment's directories were cut.
    Build { cut: bool },
    /// The prompt's list with a walk's batches added (ADR 0114), and whether it matches an
    /// input the list has not matched yet, which `RET` and `TAB` wait for. Adding rows alone
    /// leaves the selected one as it is, so they need not wait.
    Merge { awaited: bool },
}

/// No revision of an answer, so that a prompt with it is matched again.
const NO_REVISION: u64 = u64::MAX;

impl Editor {
    /// The next job for a worker, oldest first, for the host to send.
    pub fn take_completion_job(&mut self) -> Option<CompletionJob> {
        self.completion_work.jobs.pop_front()
    }

    /// Whether the innermost prompt waits for a worker's result, which its count shows as `…`
    /// and for which `RET` and `TAB` are held (ADR 0096).
    pub fn completion_pending(&self) -> bool {
        self.completion_work
            .waiting
            .as_ref()
            .is_some_and(|waiting| {
                waiting.kind != Kind::Merge { awaited: false }
                    && self
                        .prompts
                        .last()
                        .is_some_and(|prompt| prompt.id == waiting.prompt)
            })
    }

    /// Whether a job adds a walk's batches to the innermost prompt's list (ADR 0114).
    pub(in crate::editor) fn merge_job_running(&self) -> bool {
        self.completion_work
            .waiting
            .as_ref()
            .is_some_and(|waiting| {
                matches!(waiting.kind, Kind::Merge { .. })
                    && self
                        .prompts
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
                .is_some_and(|waiting| matches!(waiting.kind, Kind::Build { .. }))
    }

    /// Leaves `build` for the worker, as the innermost prompt's list, for its answer at
    /// `revision`; `cut` says a segment's directories were cut.
    pub(in crate::editor) fn leave_build(&mut self, build: Build, revision: u64, cut: bool) {
        self.leave(revision, Kind::Build { cut }, |generation, live| {
            CompletionJob::build(build, generation, live)
        });
    }

    /// Leaves `merge` for the worker, as the innermost prompt's list with a walk's batches
    /// added, for its answer at `revision`; `awaited` says it matches an input the list has not.
    pub(in crate::editor) fn leave_merge(&mut self, merge: Merge, revision: u64, awaited: bool) {
        self.leave(revision, Kind::Merge { awaited }, |generation, live| {
            CompletionJob::merge(merge, generation, live)
        });
        if !awaited {
            // The rows shown are the input's own, and stay, so the prompt waits for nothing.
            if let Some((candidates, _)) = self.completion_mut() {
                candidates.set_pending(false);
            }
        } else if let Some((candidates, _)) = self.completion_mut() {
            candidates.forget_choice();
        }
    }

    /// Leaves `matching` for the worker, for the innermost prompt's answer at `revision`.
    pub(in crate::editor) fn leave_match(&mut self, matching: Matching, revision: u64) {
        self.leave(revision, Kind::Match, |generation, live| {
            CompletionJob::matching(matching, generation, live)
        });
        // A move from here on is made over the rows shown, after the input was typed.
        if let Some((candidates, _)) = self.completion_mut() {
            candidates.forget_choice();
        }
    }

    fn leave(
        &mut self,
        revision: u64,
        kind: Kind,
        job: impl FnOnce(u64, Arc<AtomicU64>) -> CompletionJob,
    ) {
        let Some(prompt) = self.prompts.last().map(|prompt| prompt.id) else {
            return;
        };
        let work = &mut self.completion_work;
        // The job before stops at its next look, and one the host has not taken yet goes.
        let generation = work.live.fetch_add(1, Ordering::Relaxed) + 1;
        work.drop_jobs();
        work.jobs.push_back(job(generation, Arc::clone(&work.live)));
        work.waiting = Some(Waiting {
            generation,
            prompt,
            revision,
            kind,
        });
        if let Some((candidates, _)) = self.completion_mut() {
            candidates.set_pending(true);
        }
    }

    /// Leaves `garbage` for the worker to drop, as dropping it here would hold the UI thread.
    pub(in crate::editor) fn free_on_the_worker(&mut self, garbage: Box<dyn Send>) {
        self.completion_work
            .jobs
            .push_back(CompletionJob::free(garbage));
    }

    /// Frees `candidates` on the worker if they are past 5000, and here otherwise.
    pub(in crate::editor) fn free_candidates(&mut self, candidates: Candidates) {
        if candidates.total() > WORKER_PAST {
            self.free_on_the_worker(Box::new(candidates));
        }
    }

    /// Frees a closed prompt's list on the worker, if it is past 5000.
    pub(in crate::editor) fn free_prompts_list(&mut self, prompt: &mut Prompt) {
        if let Some((candidates, _)) = prompt.completion.take() {
            self.free_candidates(candidates);
        }
    }

    /// Stops the job the innermost prompt waits for, as what it was for is gone.
    pub(in crate::editor) fn stop_completion_job(&mut self) {
        let work = &mut self.completion_work;
        if work.waiting.take().is_some() {
            work.drop_jobs();
            work.live.fetch_add(1, Ordering::Relaxed);
        }
        if let Some((candidates, _)) = self.completion_mut() {
            candidates.set_pending(false);
        }
    }

    /// Gives up on the result the innermost prompt waits for, as the worker failed: the rows
    /// shown stay, and the prompt says so.
    pub fn completion_failed(&mut self) {
        if self.merge_job_running() {
            // The batches it carried still wait, for one more job (ADR 0114).
            self.stop_completion_job();
            self.merge_failed();
        } else if self.completion_pending() {
            self.stop_completion_job();
            self.prompt_says("Could not match; edit the input to try again");
        }
    }

    /// Whether a job adding a walk's batches runs for the innermost prompt's answer at
    /// `revision`, so another for it would only start over.
    pub(in crate::editor) fn merge_job_for(&self, revision: u64) -> bool {
        self.merge_job_running()
            && self
                .completion_work
                .waiting
                .as_ref()
                .is_some_and(|waiting| waiting.revision == revision)
    }

    /// Takes in what a worker built or matched, unless a newer job has taken its place, its
    /// prompt is not the innermost, or the list it matched is no longer the prompt's.
    /// Returns whether what shows changed.
    pub fn completion_reported(&mut self, report: CompletionReport) -> bool {
        let live = self
            .completion_work
            .waiting
            .as_ref()
            .is_some_and(|waiting| waiting.generation == report.0.generation());
        if !live {
            self.free_on_the_worker(Box::new(report));
            return false;
        }
        let Some(waiting) = self.completion_work.waiting.take() else {
            return false;
        };
        if self.prompts.last().map(|prompt| prompt.id) != Some(waiting.prompt) {
            self.match_again_when_innermost(&waiting);
            self.free_on_the_worker(Box::new(report));
            return false;
        }
        match report.0 {
            Report::Built { built, .. } => {
                let cut = matches!(waiting.kind, Kind::Build { cut: true });
                self.install_files(*built, cut, waiting.revision);
                // What was typed while it was built is matched now.
                self.match_completion();
            }
            Report::Matched { items, matched, .. } => {
                let Some((candidates, _)) = self.completion_mut() else {
                    return false;
                };
                candidates.set_pending(false);
                if !Arc::ptr_eq(&items, candidates.items()) {
                    self.free_on_the_worker(Box::new((items, matched)));
                    return false;
                }
                candidates.apply(matched, true);
                self.relayout_prompt();
            }
            Report::Merged { merged, .. } => {
                let Some((candidates, seen)) = self.completion_mut() else {
                    return false;
                };
                candidates.set_pending(false);
                if !Arc::ptr_eq(&merged.base, candidates.items()) {
                    self.free_on_the_worker(merged);
                    self.merge_pending_batches();
                    return false;
                }
                let merged = *merged;
                let before = candidates.grown(merged.items, merged.order, merged.matched);
                *seen = waiting.revision;
                self.free_on_the_worker(Box::new((before, merged.base)));
                self.drop_merged_batches(merged.carried);
                // The batches that came meanwhile, and what was typed since.
                self.merge_pending_batches();
                self.match_completion();
                self.relayout_prompt();
            }
        }
        // A dictionary that came while it ran is taken now (ADR 0116).
        self.completion_look_again();
        true
    }

    /// Forgets what a prompt covered by another one waited for, so that it is matched again,
    /// or its list built again, once it is the innermost again.
    fn match_again_when_innermost(&mut self, waiting: &Waiting) {
        let prompt = self
            .prompts
            .iter_mut()
            .find(|prompt| prompt.id == waiting.prompt);
        if let Some((candidates, seen)) = prompt.and_then(|prompt| prompt.completion.as_mut()) {
            candidates.set_pending(false);
            *seen = NO_REVISION;
        }
        if matches!(waiting.kind, Kind::Build { .. }) {
            self.forget_file_plan(waiting.prompt);
        }
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
            work.drop_jobs();
            work.live.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests;
