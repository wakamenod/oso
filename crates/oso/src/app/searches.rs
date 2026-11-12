//! isearch's worker (ADR 0082): one thread that runs the newest job a search left, and
//! taking in what it found and counted.
//!
//! Must not depend on: key presses or how a frame is drawn. The search and the count run on
//! the worker's thread over a snapshot, never here.
//!
//! - The thread starts with the first job and then waits for the next. A job sent while one
//!   runs supersedes it within a block of text, and jobs that queue meanwhile are dropped
//!   for the newest, on the worker, which also frees their snapshots.
//! - Nothing here logs what a worker reports: its offsets can say something of a buffer
//!   that may hold secrets.

use std::sync::mpsc::{self, Sender};
use std::thread;

use oso_core::editor::{SearchJob, SearchReport};

use super::{App, UserEvent, Waker};

/// The worker's queue, once its thread has started.
#[derive(Default)]
pub(super) struct Searcher {
    jobs: Option<Sender<SearchJob>>,
}

impl Searcher {
    /// Sends `job` to the worker, starting its thread the first time. Fails, and drops the
    /// job, if the thread cannot start or has gone.
    fn send(&mut self, job: SearchJob, waker: &Waker) -> Result<(), ()> {
        if self.jobs.is_none() {
            let (jobs, queue) = mpsc::channel::<SearchJob>();
            let waker = waker.clone();
            let spawned = thread::Builder::new()
                .name("oso-search".into())
                .spawn(move || {
                    while let Ok(mut job) = queue.recv() {
                        // Only the newest is worth running.
                        while let Ok(newer) = queue.try_recv() {
                            job = newer;
                        }
                        // It fails only once the event loop is gone.
                        job.run(|report| {
                            let _ = waker.send(UserEvent::Search(report));
                        });
                    }
                });
            if spawned.is_err() {
                return Err(());
            }
            self.jobs = Some(jobs);
        }
        match &self.jobs {
            Some(jobs) => jobs.send(job).map_err(drop),
            None => Err(()),
        }
    }
}

impl App {
    /// Keeps isearch true to its window and its text, and sends the job that leaves, if
    /// any, to the worker. Each part the job ends comes back as a [`UserEvent::Search`].
    pub(super) fn start_searches(&mut self) {
        if self.editor.isearch_follow_text() {
            self.request_redraw();
        }
        let Some(job) = self.editor.take_isearch_job() else {
            return;
        };
        if self.searcher.send(job, &self.waker).is_err() {
            // The search waits for `C-g`, and the count stays unknown.
            self.searcher = Searcher::default();
            log::warn!("the search thread could not start");
        }
    }

    /// The worker found or counted something.
    pub(super) fn isearch_reported(&mut self, report: SearchReport) {
        if self.editor.isearch_reported(report) {
            self.request_redraw();
        }
        self.start_searches();
    }
}
