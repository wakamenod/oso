//! The completion worker (ADR 0096): one thread that runs the newest job a prompt's list
//! left, building a list or matching an input past 5000 candidates, and taking in what it
//! reports.
//!
//! Must not depend on: key presses or how a frame is drawn. The build and the match run on
//! the worker's thread over a snapshot, never here.
//!
//! - The thread starts with the first job and then waits for the next. A job sent while one
//!   runs supersedes it within 1024 candidates, and jobs that queue meanwhile are dropped for
//!   the newest, on the worker, which also frees their snapshots.
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
                    while let Ok(mut job) = queue.recv() {
                        // Only the newest is worth running.
                        while let Ok(newer) = queue.try_recv() {
                            job = newer;
                        }
                        if let Some(report) = job.run() {
                            // It fails only once the event loop is gone.
                            let _ = waker.send(UserEvent::Completion(report));
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
    /// Sends the job a prompt's list left, if any, to the worker. What it comes to comes back
    /// as a [`UserEvent::Completion`].
    pub(super) fn start_completions(&mut self, event_loop: &Loop<'_>) {
        let Some(job) = self.editor.take_completion_job() else {
            return;
        };
        if let Err(job) = self.completer.send(job, &self.waker) {
            // Slower, but the prompt still answers and a `RET` held for it still runs.
            self.completer = Completer::default();
            log::warn!("the completion thread could not start; matching on the UI thread");
            if let Some(report) = job.run() {
                self.completion_reported(event_loop, report);
            }
        }
    }

    /// The worker built a list or matched an input. A `RET` or `TAB` held for it runs now.
    pub(super) fn completion_reported(&mut self, event_loop: &Loop<'_>, report: CompletionReport) {
        if self.editor.completion_reported(report) {
            self.request_redraw();
        }
        let effects = self.dispatcher.run_held_completion(&mut self.editor);
        self.apply(event_loop, effects);
        // Taking a build in can leave a match of what was typed meanwhile.
        self.start_completions(event_loop);
    }
}
