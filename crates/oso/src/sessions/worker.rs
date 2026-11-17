//! The `oso-sessions` thread (ADR 0121): it holds the rules in force and each hosted session's
//! tracker, judges a session's screen when it changed, and tells the UI thread only of changes.
//! It also reads and explains a saved screen for `session-explain-file`, reads and writes the
//! record of the sessions oso remembers, and for the issue spaces (ADR 0136) the `spaces` file,
//! their ids, their directories, whether a path is a repository, and their instructions'
//! directory.
//!
//! Must not depend on: `app` or `dispatch`; the app sends jobs and carries out what
//! comes back through the function it gives.
//!
//! Nothing runs while nothing changes: the thread waits for a job, and wakes on its own only
//! while a session's debounce or grace period waits. A session's screen is judged at most every
//! [`EVALUATE_EVERY`]; screens that come faster are coalesced, the newest kept. The app's
//! screens wait in a slot of their own, one a session, so a busy worker never queues more
//! than one screen of each.

pub mod externals;
mod jobs;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
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
    Start(SessionKey),
    /// A session's screen, title or progress changed. The app gives screens with
    /// [`Worker::send_screen`], which keeps one a session.
    Screen(SessionKey, Evidence),
    /// Screens wait in the slot: [`Worker::send_screen`] sends it, and the thread takes them.
    Screens,
    /// A session's process ended.
    Exited(SessionKey, Exit),
    /// The person has seen a session.
    Seen(SessionKey),
    /// What `git diff` found after a session's turn ended: whether it has changes that are not
    /// merged.
    Changes(SessionKey, bool),
    /// A review of a session was opened.
    ReviewOpened(SessionKey),
    /// A listing found a session's worktree clean and its branch merged, reading its changes
    /// at that instant.
    WorkMerged(SessionKey, std::time::Instant),
    /// A session is gone from oso.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "hosted sessions send it (ADR 0121 step 3)")
    )]
    Forget(SessionKey),
    /// Explain the saved screen in this file.
    ExplainFile(PathBuf),
    /// Explain the last look at a hosted session's screen (`session-explain`).
    Explain(SessionKey),
    /// Look at Claude Code's registry once a second for other programs' sessions, or stop.
    Externals(bool),
    /// With each look, `stat` these projects' `worktrees` directories, by `(root, directory)`
    /// (ADR 0131).
    Watch(Vec<(PathBuf, PathBuf)>),
    /// Read the sessions an earlier oso remembered in this state directory (ADR 0121), and the
    /// spaces (ADR 0136); with none, the spaces start empty.
    Recall(Option<PathBuf>),
    /// Remember these sessions in this state directory, keeping those of another instance:
    /// the ids not in `known`.
    Remember {
        directory: PathBuf,
        sessions: Vec<super::record::Remembered>,
        known: std::collections::HashSet<super::identity::SessionId>,
    },
    /// What the spaces ask (ADR 0136).
    Space(SpaceJob),
    /// Answer once every job handed over before this one is done, the records' writes among
    /// them: what an exit waits for (ADR 0121, "The record at an exit, as built").
    Flush(SyncSender<()>),
}

/// What the worker recalled as oso starts: the sessions an earlier oso remembered, as read,
/// and the spaces kept with them, or made from them when no spaces were kept (`migrated`), with
/// ids for new spaces (ADR 0136).
#[derive(Debug, PartialEq, Eq)]
pub struct Recalled {
    pub sessions: Vec<super::record::Remembered>,
    pub spaces: Vec<super::spaces::Stored>,
    pub migrated: bool,
    /// Whether the `spaces` file could be read, or was missing: if not, sessions are placed
    /// in this instance only, and keep the spaces their record names.
    pub readable: bool,
    pub fresh: Vec<super::spaces::SpaceId>,
    /// The sessions among them a worktree removal left behind, for the app to archive
    /// ([`super::record::stranded`], ADR 0121).
    pub stranded: Vec<super::identity::SessionId>,
}

/// What the issue spaces ask of the worker (ADR 0136): to be kept, ids for new ones, and what
/// the space commands look up on the disk.
#[derive(Debug)]
pub enum SpaceJob {
    /// Keep these spaces in this state directory, keeping those of another instance: the ids
    /// not in `known`.
    Remember {
        directory: PathBuf,
        spaces: Vec<super::spaces::Stored>,
        known: std::collections::HashSet<super::spaces::SpaceId>,
    },
    /// Read this many ids for new spaces.
    Ids(usize),
    /// Make the directory of space `id`, called `name`, under `root`, for its `members`,
    /// under the next free name if that one is taken, here or by a name in `taken` (step
    /// 1b); and look at the members' devices and inodes.
    MakeDirectory {
        id: super::spaces::SpaceId,
        root: PathBuf,
        name: String,
        members: Vec<super::spaces::Member>,
        taken: Vec<String>,
        /// Made under `name` alone, as a directory gone from its recorded place is made again
        /// there (`C-u space-orchestrator-start`).
        exact: bool,
    },
    /// Whether `path` is a repository's main checkout (`space-add-repo`, `space-new`).
    CheckRepository {
        check: super::spaces::probe::RepoCheck,
        path: PathBuf,
    },
    /// The file of space `id`'s instructions in the state directory `state`, its directory
    /// made private (`space-instructions`).
    Instructions {
        id: super::spaces::SpaceId,
        state: PathBuf,
    },
    /// Whether space `id`'s `handoff` file is there, and where session `from`'s transcript is,
    /// for its orchestrator's replacement (ADR 0136, step 5).
    Handover {
        id: super::spaces::SpaceId,
        handoff: PathBuf,
        from: super::identity::SessionId,
    },
}

/// What the worker tells the app.
#[derive(Debug, PartialEq, Eq)]
pub enum Out {
    /// A session's state changed.
    Changed(SessionKey, Change),
    /// The live sessions of Claude Code's registry changed: all of them, the app's own too.
    Externals(Vec<externals::External>),
    /// These watched projects' worktrees changed: they are listed again (ADR 0131).
    WorktreesMoved(Vec<PathBuf>),
    /// A session's recap changed (ADR 0121, step 3): cleaned and cut, the screen's text.
    Recap(SessionKey, String),
    /// Whether a hold rule matched a session's newest look changed, or `None`, the newest
    /// look ran out of time and its state holds the last (ADR 0121, step 5): a send waits
    /// for a look judged in time, and is refused while a full-screen view holds the state.
    Judged(SessionKey, Option<bool>),
    /// A session's screen shows a message from the session of this name that the last look
    /// did not (ADR 0121, step 5): cleaned and cut, the screen's text.
    Message(SessionKey, String),
    /// The explanation of a saved screen, for `*Session explain*`: of session `key`'s last
    /// look, or of a file for `None`.
    Explained(Option<SessionKey>, String),
    /// A saved screen could not be read, and why, for the echo area, of session `key`'s or a
    /// file's. It names no path: what the person typed stays theirs, and the agent channel reads
    /// the echo area.
    Unexplained(Option<SessionKey>, String),
    /// The rules took longer than their budget on [`SLOW_AFTER`] looks in a row at a screen,
    /// this rule the longest: said once for each set of rules.
    Slow(String),
    /// The sessions an earlier oso remembered, as read, and the spaces kept with them, or made
    /// from them when no spaces were kept (ADR 0136).
    Recalled(Recalled),
    /// Ids for new spaces, as asked.
    SpaceIds(Vec<super::spaces::SpaceId>),
    /// Space `id`'s directory was made, or why not, and its members' devices and inodes.
    SpaceDirMade {
        id: super::spaces::SpaceId,
        made: Result<super::spaces::probe::Made, String>,
        identities: Vec<(PathBuf, Option<(u64, u64)>)>,
    },
    /// What `path` was found to be, for the check `check`.
    RepositoryChecked {
        check: super::spaces::probe::RepoCheck,
        path: PathBuf,
        member: Result<super::spaces::Member, String>,
    },
    /// Space `id`'s instructions' file, or why its directory may not be used.
    InstructionsReady {
        id: super::spaces::SpaceId,
        file: Result<PathBuf, String>,
    },
    /// What a replacement of space `id`'s orchestrator looked at: whether its handoff file is
    /// a file there, and the old orchestrator's transcript, if found.
    HandoverLooked {
        id: super::spaces::SpaceId,
        from: super::identity::SessionId,
        handoff: bool,
        transcript: Option<PathBuf>,
    },
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
    pub fn start(out: impl Fn(Out) + Send + Sync + 'static) -> Option<Self> {
        let (jobs, receiver) = mpsc::channel();
        let screens = Slot::default();
        let slot = Arc::clone(&screens);
        let out: Arc<dyn Fn(Out) + Send + Sync> = Arc::new(out);
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

    /// Waits, at most `within`, until the thread has done every job handed over before now:
    /// the sessions' and the spaces' records are then written, or their failure logged.
    /// Whether it said so in time; a thread that ended, or failed that batch, says nothing.
    pub fn flush(&self, within: Duration) -> bool {
        let (done, answer) = mpsc::sync_channel(1);
        self.send(Job::Flush(done));
        answer.recv_timeout(within).is_ok()
    }
}

/// The thread: waits for jobs, and for the next time a session is due, until the worker is
/// dropped. A panic in one job is logged and the thread goes on, its sessions as they were.
fn run(jobs: &Receiver<Job>, screens: &Slot, out: &Arc<dyn Fn(Out) + Send + Sync>) {
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
                    Job::Space(probe) if jobs::looks_at_the_disk(&probe) => {
                        probe_apart(probe, out);
                    }
                    job => found.extend(core.handle(job, now)),
                }
            }
            found.extend(core.tick(now));
        }));
        found.into_iter().for_each(|found| out(found));
        if done.is_err() {
            log::error!("the sessions worker failed a job");
        }
    }
}

/// Does what a space command asked to look up on the disk on a thread of its own, so that a
/// path on a mount that does not answer never holds the judging of screens (ADR 0136, step 3).
fn probe_apart(probe: SpaceJob, out: &Arc<dyn Fn(Out) + Send + Sync>) {
    let out = Arc::clone(out);
    let spawned = thread::Builder::new()
        .name("oso-space-probe".into())
        .spawn(move || {
            jobs::space_job(probe)
                .into_iter()
                .for_each(|found| out(found))
        });
    if let Err(e) = spawned {
        log::warn!("a space's look at the disk cannot start: {e}");
    }
}

/// How long the start waits for [`stranded_apart`]'s look: past it, no session is taken as
/// stranded this time, so a mount that does not answer never holds the recall, nor the jobs
/// after it (ADR 0121, "Removal archives its sessions").
const STRANDED_WAIT: Duration = Duration::from_millis(250);

/// [`record::stranded`](super::record::stranded) on a thread of its own, waited for at most
/// [`STRANDED_WAIT`]: past it, or with no thread, none, and the log says so. The thread of a
/// look that does not answer is left to end.
fn stranded_apart(candidates: Vec<super::record::Candidate>) -> Vec<super::identity::SessionId> {
    if candidates.is_empty() {
        return Vec::new();
    }
    let (send, answer) = mpsc::sync_channel(1);
    let spawned = thread::Builder::new()
        .name("oso-sessions-stranded".into())
        .spawn(move || {
            let _ = send.send(super::record::stranded(&candidates));
        });
    if let Err(e) = spawned {
        log::warn!("the look for sessions a removed worktree left cannot start: {e}");
        return Vec::new();
    }
    answer.recv_timeout(STRANDED_WAIT).unwrap_or_else(|_| {
        log::warn!("the look for sessions a removed worktree left took over {STRANDED_WAIT:?}");
        Vec::new()
    })
}

/// Hands the screens in the slot to `core`, emptying it.
fn take_screens(core: &mut Core, screens: &Slot, now: Instant) {
    let taken = std::mem::take(&mut *screens.lock().unwrap_or_else(PoisonError::into_inner));
    for (key, evidence) in taken {
        // A screen changes nothing to report until a tick judges it.
        core.handle(Job::Screen(key, evidence), now);
    }
}
