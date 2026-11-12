//! The completion worker (ADR 0096): one thread that runs the newest job a prompt's list
//! left, building a list or matching an input past 5000 candidates, and taking in what it
//! reports.
//!
//! Must not depend on: key presses or how a frame is drawn. The build and the match run on
//! the worker's thread over a snapshot, never here.
//!
//! - The thread starts with the first job and then runs each in turn. A job superseded before
//!   it runs stops at its first look, and one superseded while it runs within 1024 candidates,
//!   so jobs run in order cost the newest one little, and a job that only frees what a prompt
//!   no longer holds is never skipped.
//! - A job that panics is reported, so the prompt stops waiting and a held key is dropped; the
//!   thread goes on. The panic hook writes no message, so nothing of the list is printed.
//! - It shares nothing with isearch's worker: a prompt and isearch never run at once, and each
//!   keeps its jobs and its generation its own.
//! - Nothing here logs what a worker reports: a list can name files the person keeps private.

use std::sync::mpsc::{self, Sender};
use std::thread;

use oso_core::complete::{CompletionJob, CompletionReport};

use super::{App, Loop, UserEvent, Waker};

/// The worker's queue, once its thread has started.
#[derive(Default)]
pub(super) struct Completer {
    jobs: Option<Sender<CompletionJob>>,
}

impl Completer {
    /// Sends `job` to the worker, starting its thread the first time. Gives the job back if
    /// the thread cannot start or has gone.
    fn send(&mut self, job: CompletionJob, waker: &Waker) -> Result<(), CompletionJob> {
        if self.jobs.is_none() {
            let (jobs, queue) = mpsc::channel::<CompletionJob>();
            let waker = waker.clone();
            let spawned = thread::Builder::new()
                .name("oso-complete".into())
                .spawn(move || {
                    while let Ok(job) = queue.recv() {
                        let ran =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job.run()));
                        // A send fails only once the event loop is gone.
                        match ran {
                            Ok(Some(report)) => {
                                let _ = waker.send(UserEvent::Completion(report));
                            }
                            Ok(None) => {}
                            Err(_) => {
                                let _ = waker.send(UserEvent::CompletionFailed);
                            }
                        }
                    }
                });
            if spawned.is_err() {
                return Err(job);
            }
            self.jobs = Some(jobs);
        }
        match &self.jobs {
            Some(jobs) => jobs.send(job).map_err(|failed| failed.0),
            None => Err(job),
        }
    }
}

impl App {
    /// Sends the jobs a prompt's list left, if any, to the worker, in order. What they come to
    /// comes back as a [`UserEvent::Completion`].
    pub(super) fn start_completions(&mut self, event_loop: &Loop<'_>) {
        while let Some(job) = self.editor.take_completion_job() {
            if let Err(job) = self.completer.send(job, &self.waker) {
                // Slower, but the prompt still answers and a `RET` held for it still runs.
                self.completer = Completer::default();
                log::warn!("the completion thread could not start; matching on the UI thread");
                if let Some(report) = job.run() {
                    self.completion_reported(event_loop, report);
                }
            }
        }
    }

    /// The worker built a list or matched an input. A `RET` or `TAB` held for it runs now.
    pub(super) fn completion_reported(&mut self, event_loop: &Loop<'_>, report: CompletionReport) {
        if self.editor.completion_reported(report) {
            self.request_redraw();
        }
        self.run_held_completion(event_loop);
        // Taking a build in can leave a match of what was typed meanwhile.
        self.start_completions(event_loop);
    }

    /// A job panicked on the worker: the prompt stops waiting, and says so, and a key held for
    /// it is dropped.
    pub(super) fn completion_failed(&mut self) {
        log::warn!("a completion job panicked");
        self.dispatcher.drop_held_completion();
        self.editor.completion_failed();
        self.request_redraw();
    }

    /// Runs a `RET`, `M-RET` or `TAB` held for a result once the prompt waits for nothing more,
    /// as a report, a list built here or a listing ends the wait. Never once the app is leaving.
    pub(super) fn run_held_completion(&mut self, event_loop: &Loop<'_>) {
        if self.is_leaving(event_loop) {
            self.dispatcher.drop_held_completion();
            return;
        }
        let effects = self.dispatcher.run_held_completion(&mut self.editor);
        self.apply(event_loop, effects);
    }
}
