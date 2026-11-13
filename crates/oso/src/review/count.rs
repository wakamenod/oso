//! The review menu's counts (ADR 0122, ADR 0117 "Live content"): how many files each scope
//! differs in, listed as a review lists them, on at most [`THREADS`] threads, `oso-review-count`.
//!
//! Must not depend on: `app`, `dispatch`, winit or the UI thread; the caller hands jobs in and
//! takes counts out, and supplies the wake-up.
//!
//! - A job runs only from a [`TrustedRoot`], as a review does, under the same limits: each git
//!   run is bounded and killed past 10 s, and no file's sides are read.
//! - Each opening of the menu is an epoch. A new one, or [`Counter::stop`], drops the jobs
//!   waiting, and a job running stops between its git runs and sends nothing.
//! - The branch's base is found with its count, from the branch's upstream, else
//!   `origin/HEAD`'s branch, else `main` or `master`.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread;

use oso_core::editor::{Count, ScopeKind};
use oso_core::project::ProjectId;

use super::scope::ScopeError;
use super::work::Reader;
use super::{Scope, TrustedRoot};

/// The most threads counting at once.
pub(crate) const THREADS: usize = 2;

/// What a job counts: a scope, or the branch against its base, found first.
pub(crate) enum Counting {
    Scope(ScopeKind, Scope),
    Branch,
}

/// One count to make, for an opening of the menu.
pub(crate) struct Job {
    pub(crate) generation: u64,
    pub(crate) project: ProjectId,
    pub(crate) root: TrustedRoot,
    pub(crate) counting: Counting,
}

/// A count made: of which opening, project and scope, and the branch's base when it was found.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Counted {
    pub(crate) generation: u64,
    pub(crate) project: ProjectId,
    pub(crate) scope: ScopeKind,
    pub(crate) count: Count,
    pub(crate) base: Option<Option<String>>,
}

/// Hands a count to the event loop.
type SendCount = Arc<dyn Fn(Counted) + Send + Sync>;

/// What the threads share.
struct Shared {
    queue: Mutex<VecDeque<(u64, Job)>>,
    ready: Condvar,
    epoch: AtomicU64,
    live: AtomicBool,
}

/// The counting threads, started with the first jobs. Dropping it ends them.
pub(crate) struct Counter {
    shared: Arc<Shared>,
    state: Option<PathBuf>,
    send: SendCount,
    started: bool,
}

impl Counter {
    /// A counter that makes its index copies under `state` and hands each count to `send`.
    pub(crate) fn new(
        state: Option<PathBuf>,
        send: impl Fn(Counted) + Send + Sync + 'static,
    ) -> Self {
        Counter {
            shared: Arc::new(Shared {
                queue: Mutex::new(VecDeque::new()),
                ready: Condvar::new(),
                epoch: AtomicU64::new(0),
                live: AtomicBool::new(true),
            }),
            state,
            send: Arc::new(send),
            started: false,
        }
    }

    /// Starts a new epoch with `jobs`: those of the one before are dropped or stopped.
    pub(crate) fn start(&mut self, jobs: Vec<Job>) {
        self.start_threads();
        let epoch = self.shared.epoch.fetch_add(1, Ordering::SeqCst) + 1;
        let mut queue = self
            .shared
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        queue.clear();
        queue.extend(jobs.into_iter().map(|job| (epoch, job)));
        drop(queue);
        self.shared.ready.notify_all();
    }

    /// Adds `jobs` to the epoch running, as a count the menu needs once a baseline came, or
    /// starts one if none ran yet.
    pub(crate) fn add(&mut self, jobs: Vec<Job>) {
        if !self.started {
            return self.start(jobs);
        }
        let epoch = self.shared.epoch.load(Ordering::SeqCst);
        self.shared
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend(jobs.into_iter().map(|job| (epoch, job)));
        self.shared.ready.notify_all();
    }

    /// Drops the jobs waiting, and stops those running, as when the menu closed.
    pub(crate) fn stop(&self) {
        self.shared.epoch.fetch_add(1, Ordering::SeqCst);
        self.shared
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }

    fn start_threads(&mut self) {
        if self.started {
            return;
        }
        self.started = true;
        for _ in 0..THREADS {
            let shared = Arc::clone(&self.shared);
            let send = Arc::clone(&self.send);
            let state = self.state.clone();
            let started = thread::Builder::new()
                .name("oso-review-count".into())
                .spawn(move || serve(&shared, &*send, state));
            if let Err(e) = started {
                log::warn!("a thread to count the review menu's files could not start: {e}");
            }
        }
    }
}

impl Drop for Counter {
    fn drop(&mut self) {
        self.shared.live.store(false, Ordering::SeqCst);
        self.stop();
        self.shared.ready.notify_all();
    }
}

fn serve(shared: &Shared, send: &(dyn Fn(Counted) + Send + Sync), state: Option<PathBuf>) {
    loop {
        let (epoch, job) = {
            let mut queue = shared.queue.lock().unwrap_or_else(PoisonError::into_inner);
            loop {
                if !shared.live.load(Ordering::SeqCst) {
                    return;
                }
                if let Some(next) = queue.pop_front() {
                    break next;
                }
                queue = shared
                    .ready
                    .wait(queue)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        };
        let current = || shared.epoch.load(Ordering::SeqCst) == epoch;
        if !current() {
            continue;
        }
        let counted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            count(&job, state.as_deref(), &current)
        }));
        match counted {
            Ok(Some(counted)) if current() => send(counted),
            Ok(_) => {}
            Err(_) => log::warn!("counting a review scope's files panicked"),
        }
    }
}

/// Makes `job`'s count, or none once `current` says its epoch is over.
fn count(
    job: &Job,
    state: Option<&std::path::Path>,
    current: &dyn Fn() -> bool,
) -> Option<Counted> {
    let failed = |scope| Counted {
        generation: job.generation,
        project: job.project,
        scope,
        count: Count::Failed,
        base: None,
    };
    if job.root.revoked() || !job.root.still_there() {
        let scope = match &job.counting {
            Counting::Scope(kind, _) => *kind,
            Counting::Branch => ScopeKind::Branch,
        };
        return Some(failed(scope));
    }
    let mut reader = Reader::new(job.root.path(), state);
    let (kind, scope, base) = match &job.counting {
        Counting::Scope(kind, scope) => (*kind, scope.clone(), None),
        Counting::Branch => match reader.branch_base() {
            Ok(Some(base)) => (
                ScopeKind::Branch,
                Scope::Branch { base: base.clone() },
                Some(Some(base)),
            ),
            Ok(None) => {
                return Some(Counted {
                    base: Some(None),
                    ..failed(ScopeKind::Branch)
                });
            }
            Err(_) => return Some(failed(ScopeKind::Branch)),
        },
    };
    // Between each of its runs of git, the count stops once its opening is over or the
    // project is no longer trusted.
    let stop = || !current() || job.root.revoked();
    if stop() {
        return None;
    }
    let count = match reader.count(&scope, &stop) {
        Ok((_, more)) if more > 0 => Count::More,
        Ok((files, _)) => Count::Files(files),
        Err(ScopeError::Stopped) => return None,
        Err(_) => Count::Failed,
    };
    Some(Counted {
        generation: job.generation,
        project: job.project,
        scope: kind,
        count,
        base,
    })
}

#[cfg(test)]
mod tests;
