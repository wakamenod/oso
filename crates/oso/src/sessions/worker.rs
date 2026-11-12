//! The `oso-sessions` thread (ADR 0121): it holds the rules in force and each hosted session's
//! tracker, judges a session's screen when it changed, and tells the UI thread only of changes.
//! It also reads and explains a saved screen for `session-explain-file`.
//!
//! Must not depend on: `app`, `commands` or `dispatch`; the app sends jobs and carries out what
//! comes back through the function it gives.
//!
//! Nothing runs while nothing changes: the thread waits for a job, and wakes on its own only
//! while a session's debounce or grace period waits. A session's screen is judged at most every
//! [`EVALUATE_EVERY`]; screens that come faster are coalesced, the newest kept.

mod jobs;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use super::rules::RuleSet;
use super::screen::Evidence;
use super::state::{Change, Exit};

use jobs::Core;

/// The least time between two judgements of one session's screen: at most 10 a second.
pub const EVALUATE_EVERY: Duration = Duration::from_millis(100);

/// A hosted session, as the worker knows it: a number the app gives.
pub type SessionKey = u64;

/// What the app asks of the worker.
#[derive(Debug)]
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "sessions and the terminal's host API feed these (ADR 0121 step 3)"
    )
)]
pub enum Job {
    /// The rules in force from now, for every session (ADR 0121, Q4).
    Rules(Arc<RuleSet>),
    /// A session started or was resumed.
    Start(SessionKey),
    /// A session's screen, title or progress changed.
    Screen(SessionKey, Evidence),
    /// A session's process ended.
    Exited(SessionKey, Exit),
    /// The person has seen a session.
    Seen(SessionKey),
    /// What `git diff` found after a session's turn ended.
    Changes(SessionKey, bool),
    /// A review of a session was opened.
    ReviewOpened(SessionKey),
    /// A session is gone from oso.
    Forget(SessionKey),
    /// Explain the saved screen in this file.
    ExplainFile(PathBuf),
}

/// What the worker tells the app.
#[derive(Debug, PartialEq, Eq)]
pub enum Out {
    /// A session's state changed.
    Changed(SessionKey, Change),
    /// The explanation of a saved screen, for `*Session explain*`.
    Explained(String),
    /// A saved screen could not be read, and why, for the echo area.
    Unexplained(String),
}

/// The worker, which stops when this is dropped.
pub struct Worker {
    jobs: Sender<Job>,
}

impl Worker {
    /// Starts the thread with the bundled rules; it calls `out` with what it finds. `None`, said
    /// in the log, if the thread cannot start.
    pub fn start(out: impl Fn(Out) + Send + 'static) -> Option<Self> {
        let (jobs, receiver) = mpsc::channel();
        let spawned = thread::Builder::new()
            .name("oso-sessions".into())
            .spawn(move || run(&receiver, &out));
        if let Err(e) = spawned {
            log::warn!("cannot start the sessions worker: {e}");
            return None;
        }
        Some(Self { jobs })
    }

    /// Hands `job` to the thread. A thread that ended takes nothing, which its end logged.
    pub fn send(&self, job: Job) {
        let _ = self.jobs.send(job);
    }
}

/// The thread: waits for jobs, and for the next time a session is due, until the worker is
/// dropped. A panic in one job is logged and the thread goes on, its sessions as they were.
fn run(jobs: &Receiver<Job>, out: &dyn Fn(Out)) {
    let mut core = Core::new(Arc::new(RuleSet::bundled()));
    loop {
        let job = match core.due() {
            Some(due) => {
                let wait = due.saturating_duration_since(Instant::now());
                match jobs.recv_timeout(wait) {
                    Ok(job) => Some(job),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
            None => match jobs.recv() {
                Ok(job) => Some(job),
                Err(_) => return,
            },
        };
        let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let now = Instant::now();
            let mut found = job.map(|job| core.handle(job, now)).unwrap_or_default();
            found.extend(core.tick(now));
            found
        }));
        match done {
            Ok(found) => found.into_iter().for_each(out),
            Err(_) => log::error!("the sessions worker failed a job"),
        }
    }
}
