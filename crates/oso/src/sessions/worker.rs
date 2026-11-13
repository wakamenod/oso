//! The `oso-sessions` thread (ADR 0121): it holds the rules in force and each hosted session's
//! tracker, judges a session's screen when it changed, and tells the UI thread only of changes.
//! It also reads and explains a saved screen for `session-explain-file`, and reads and writes
//! the record of the sessions oso remembers.
//!
//! Must not depend on: `app`, `commands` or `dispatch`; the app sends jobs and carries out what
//! comes back through the function it gives.
//!
//! Nothing runs while nothing changes: the thread waits for a job, and wakes on its own only
//! while a session's debounce or grace period waits. A session's screen is judged at most every
//! [`EVALUATE_EVERY`]; screens that come faster are coalesced, the newest kept. The app's
//! screens wait in a slot of their own, one a session, so a busy worker never queues more
//! than one screen of each.

mod jobs;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use super::rules::RuleSet;
use super::screen::Evidence;
use super::state::{Change, Exit};

use jobs::Core;

/// The least time between two judgements of one session's screen: at most 10 a second.
pub const EVALUATE_EVERY: Duration = Duration::from_millis(100);

/// How many looks in a row at one session's screen must run out of time before `*Messages*`
/// says the rules are too slow: the deadline is the wall clock's, which one busy moment of the
/// machine can pass.
pub const SLOW_AFTER: u32 = 2;

/// How many looks in a row a local rule may be the slowest of looks that ran out of time
/// before the worker stops using it, until the rules load again. A bundled rule is never
/// dropped: its cost is bounded and tested, and a busy machine must not turn a dialog off.
pub const DROP_AFTER: u32 = 3;

/// A hosted session, as the worker knows it: a number the app gives.
pub type SessionKey = u64;

/// What the app asks of the worker.
#[derive(Debug)]
pub enum Job {
    /// The rules in force from now, for every session (ADR 0121, Q4).
    Rules(Arc<RuleSet>),
    /// A session started or was resumed.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "hosted sessions start it (ADR 0121 step 3)")
    )]
    Start(SessionKey),
    /// A session's screen, title or progress changed. The app gives screens with
    /// [`Worker::send_screen`], which keeps one a session.
    Screen(SessionKey, Evidence),
    /// Screens wait in the slot: [`Worker::send_screen`] sends it, and the thread takes them.
    Screens,
    /// A session's process ended.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the terminal's thread reports it (ADR 0121 step 3)"
        )
    )]
    Exited(SessionKey, Exit),
    /// The person has seen a session.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the 2 s seen timer sends it (ADR 0121 step 3)")
    )]
    Seen(SessionKey),
    /// What `git diff` found after a session's turn ended.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the `git diff` call sends it (ADR 0121 step 3)")
    )]
    Changes(SessionKey, bool),
    /// A review of a session was opened.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "opening a review sends it (ADR 0121 step 3)")
    )]
    ReviewOpened(SessionKey),
    /// A session is gone from oso.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "hosted sessions send it (ADR 0121 step 3)")
    )]
    Forget(SessionKey),
    /// Explain the saved screen in this file.
    ExplainFile(PathBuf),
    /// Read the sessions an earlier oso remembered in this state directory (ADR 0121).
    Recall(PathBuf),
    /// Remember these sessions in this state directory, keeping those of another instance:
    /// the ids not in `known`.
    Remember {
        directory: PathBuf,
        sessions: Vec<super::record::Remembered>,
        known: std::collections::HashSet<super::identity::SessionId>,
    },
}

/// What the worker tells the app.
#[derive(Debug, PartialEq, Eq)]
pub enum Out {
    /// A session's state changed.
    Changed(SessionKey, Change),
    /// A session's recap changed (ADR 0121, step 3): cleaned and cut, the screen's text.
    Recap(SessionKey, String),
    /// The explanation of a saved screen, for `*Session explain*`.
    Explained(String),
    /// A saved screen could not be read, and why, for the echo area. It names no path: what
    /// the person typed stays theirs, and the agent channel reads the echo area.
    Unexplained(String),
    /// The rules took longer than their budget on [`SLOW_AFTER`] looks in a row at a screen,
    /// this rule the longest: said once for each set of rules.
    Slow(String),
    /// The sessions an earlier oso remembered, as read.
    Recalled(Vec<super::record::Remembered>),
    /// This local rule was the slowest of [`DROP_AFTER`] late looks in a row, so it is no
    /// longer used until the rules load again.
    Dropped(String),
}

/// The screens the app gave that the thread has not taken, one a session, the newest.
type Slot = Arc<Mutex<HashMap<SessionKey, Evidence>>>;

/// The worker, which stops when this is dropped.
pub struct Worker {
    jobs: Sender<Job>,
    screens: Slot,
}

impl Worker {
    /// Starts the thread with the bundled rules; it calls `out` with what it finds. `None`, said
    /// in the log, if the thread cannot start.
    pub fn start(out: impl Fn(Out) + Send + 'static) -> Option<Self> {
        let (jobs, receiver) = mpsc::channel();
        let screens = Slot::default();
        let slot = Arc::clone(&screens);
        let spawned = thread::Builder::new()
            .name("oso-sessions".into())
            .spawn(move || run(&receiver, &slot, &out));
        if let Err(e) = spawned {
            log::warn!("cannot start the sessions worker: {e}");
            return None;
        }
        Some(Self { jobs, screens })
    }

    /// Hands session `key`'s new screen to the thread, in place of one it has not taken yet.
    pub fn send_screen(&self, key: SessionKey, evidence: Evidence) {
        let first = {
            let mut screens = self.screens.lock().unwrap_or_else(PoisonError::into_inner);
            let empty = screens.is_empty();
            screens.insert(key, evidence);
            empty
        };
        // The thread takes every screen in the slot at once: one job wakes it.
        if first {
            self.send(Job::Screens);
        }
    }

    /// Hands `job` to the thread. A thread that ended takes nothing, which its end logged.
    pub fn send(&self, job: Job) {
        let _ = self.jobs.send(job);
    }
}

/// The thread: waits for jobs, and for the next time a session is due, until the worker is
/// dropped. A panic in one job is logged and the thread goes on, its sessions as they were.
fn run(jobs: &Receiver<Job>, screens: &Slot, out: &dyn Fn(Out)) {
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
        // What the jobs before a panic found still goes out: a change of state is not lost
        // with the job that failed.
        let mut found = Vec::new();
        let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let now = Instant::now();
            // Every job waiting goes in before a tick, so that screens queued while the last
            // tick ran are coalesced, the newest kept, and judged once.
            for job in job
                .into_iter()
                .chain(std::iter::from_fn(|| jobs.try_recv().ok()))
            {
                match job {
                    Job::Screens => take_screens(&mut core, screens, now),
                    job => found.extend(core.handle(job, now)),
                }
            }
            found.extend(core.tick(now));
        }));
        found.into_iter().for_each(out);
        if done.is_err() {
            log::error!("the sessions worker failed a job");
        }
    }
}

/// Hands the screens in the slot to `core`, emptying it.
fn take_screens(core: &mut Core, screens: &Slot, now: Instant) {
    let taken = std::mem::take(&mut *screens.lock().unwrap_or_else(PoisonError::into_inner));
    for (key, evidence) in taken {
        // A screen changes nothing to report until a tick judges it.
        core.handle(Job::Screen(key, evidence), now);
    }
}
