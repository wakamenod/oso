//! Soft wrap's worker (ADR 0100, ADR 0103): one thread that runs the jobs the windows' wraps
//! hand out, and taking in what they wrapped.
//!
//! Must not depend on: key presses or how a frame is drawn. The rows are wrapped on the
//! worker's thread over a snapshot, never here.
//!
//! - The thread starts with the first job and then waits for the next. A job a newer one of
//!   its window supersedes stops at its next check, and one whose window dropped its wrap
//!   stops too, so the queue moves on.
//! - Nothing here logs what a worker reports: where rows break says how long words are in a
//!   buffer that may hold secrets.

use std::panic::{self, AssertUnwindSafe};
use std::sync::mpsc::{self, Sender};
use std::thread;

use oso_core::display_map::wrap_map::{WrapJob, WrapReport};

use super::{App, UserEvent, Waker};

/// The worker's queue, once its thread has started.
#[derive(Default)]
pub(super) struct Wrapper {
    jobs: Option<Sender<WrapJob>>,
}

impl Wrapper {
    /// Sends `job` to the worker, starting its thread the first time. Fails, and drops the
    /// job, if the thread cannot start or has gone.
    fn send(&mut self, job: WrapJob, waker: &Waker) -> Result<(), ()> {
        if self.jobs.is_none() {
            let (jobs, queue) = mpsc::channel::<WrapJob>();
            let waker = waker.clone();
            let spawned = thread::Builder::new()
                .name("oso-wrap".into())
                .spawn(move || {
                    while let Ok(job) = queue.recv() {
                        // A job that panics leaves its window's rows provisional, and the
                        // thread goes on with the others.
                        match panic::catch_unwind(AssertUnwindSafe(|| job.run())) {
                            Ok(Some(report)) => {
                                // It fails only once the event loop is gone.
                                let _ = waker.send(UserEvent::Wrap(report));
                            }
                            Ok(None) => {}
                            Err(_) => log::warn!("a wrap job panicked"),
                        }
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
    /// Sends the jobs the windows' wraps want to the worker. Each report comes back as a
    /// [`UserEvent::Wrap`].
    pub(super) fn start_wraps(&mut self) {
        for job in self.editor.take_wrap_jobs() {
            if self.wrapper.send(job, &self.waker).is_err() {
                // The rows near the view still wrap on this thread; the rest stay
                // provisional.
                self.wrapper = Wrapper::default();
                log::warn!("the wrap thread could not start");
                return;
            }
        }
    }

    /// The worker wrapped what a window's wrap wanted.
    pub(super) fn wrap_reported(&mut self, report: WrapReport) {
        if self.editor.take_wrap_report(report) {
            self.request_redraw();
        }
        self.start_wraps();
    }
}
