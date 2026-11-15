//! The `oso-files` thread (ADR 0134, step 1b-2): it runs one file operation at a time, in the
//! order asked, and tells the UI how far each got, at most every 100 ms once one has run for
//! 500 ms, and how it ended.
//!
//! Must not depend on: `app`, `dispatch`, `commands`, `lister` or winit; the caller supplies the
//! wake-up.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use super::{Outcome, Plan, Progress, run};

/// How long an operation runs before it says how far it got, and how often then.
const QUIET: Duration = Duration::from_millis(500);
const EVERY: Duration = Duration::from_millis(100);

/// What the thread tells the UI.
#[derive(Debug)]
pub(crate) enum Report {
    /// The operation `id` has done `done` entries so far.
    Progress { id: u64, done: usize },
    /// The operation `id` ended so.
    Finished { id: u64, outcome: Outcome },
}

/// The thread's side of the UI: its reports, and whether to stop the operation running.
#[derive(Default)]
struct Shared {
    reports: Mutex<Vec<Report>>,
    stop: AtomicBool,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The thread, started with the first operation. Dropping it lets it end once its operation
/// has.
pub(crate) struct Worker {
    jobs: Option<Sender<(u64, Plan)>>,
    shared: Arc<Shared>,
    wake: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl Worker {
    /// A worker that calls `wake` after each report, which says false once the UI is gone.
    pub(crate) fn new(wake: impl Fn() -> bool + Send + Sync + 'static) -> Self {
        Self {
            jobs: None,
            shared: Arc::default(),
            wake: Arc::new(wake),
        }
    }

    /// Runs `plan` once those asked before have ended, as the operation `id`.
    pub(crate) fn run(&mut self, id: u64, plan: Plan) -> Result<(), String> {
        let jobs = match &self.jobs {
            Some(jobs) => jobs.clone(),
            None => {
                let (jobs, received) = mpsc::channel::<(u64, Plan)>();
                let shared = Arc::clone(&self.shared);
                let wake = Arc::clone(&self.wake);
                thread::Builder::new()
                    .name("oso-files".into())
                    .spawn(move || {
                        for (id, plan) in received {
                            serve(&shared, wake.as_ref(), id, &plan);
                        }
                    })
                    .map_err(|error| {
                        format!("cannot start the file operations' thread: {error}")
                    })?;
                self.jobs = Some(jobs.clone());
                jobs
            }
        };
        self.shared.stop.store(false, Ordering::Relaxed);
        jobs.send((id, plan))
            .map_err(|_| "the file operations' thread has gone".to_owned())
    }

    /// Asks the operation running to stop after the entry in hand.
    pub(crate) fn stop(&self) {
        self.shared.stop.store(true, Ordering::Relaxed);
    }

    /// What the thread told since the last call, oldest first.
    pub(crate) fn take(&self) -> Vec<Report> {
        std::mem::take(&mut *lock(&self.shared.reports))
    }
}

fn serve(shared: &Shared, wake: &(dyn Fn() -> bool + Send + Sync), id: u64, plan: &Plan) {
    let started = Instant::now();
    let mut next = started + QUIET;
    let stop = || shared.stop.load(Ordering::Relaxed);
    let mut each = |done: usize| {
        if Instant::now() >= next {
            next = Instant::now() + EVERY;
            lock(&shared.reports).push(Report::Progress { id, done });
            wake();
        }
    };
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut progress = Progress {
            stop: &stop,
            each: &mut each,
        };
        run(plan, &mut progress)
    }))
    .unwrap_or_else(|_| Outcome {
        error: Some("the operation failed part way".to_owned()),
        ..Outcome::default()
    });
    lock(&shared.reports).push(Report::Finished { id, outcome });
    wake();
}
