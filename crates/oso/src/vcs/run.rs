//! A repository's thread for git's long runs, `oso-vcs-run` (ADR 0127, step 1b): a commit with
//! the repository's hooks, which has no time limit, the message an amend starts from, and, from
//! step 1c, reading the record of what discards kept and writing a copy back; from step 2,
//! pushes, pulls and fetches (`remotes`), and planning where a push goes; from step 3a,
//! continuing, skipping and aborting what is stopped (`sequence`); from step 3b, stashing
//! (`stash`).
//!
//! Must not depend on: `app`, `dispatch`, winit or the UI thread; the caller hands requests in,
//! takes reports out, and supplies the wake-up.
//!
//! - It starts only from a [`TrustedRoot`], looks at the root before each request, and finds
//!   git as the status's thread does: the repository's top must be the root. A run stops once
//!   the root is revoked, as once the person stops it.
//! - A commit runs the repository's own hooks, with the environment `git`'s runner gives a
//!   hook and the names of `[git] pass_env`, in a process group of its own, so that a stop
//!   ends what a hook started too. No `GIT_LITERAL_PATHSPECS`: a hook's own git takes its
//!   paths as patterns.
//! - What git writes streams out as lines a log shows ([`commit::log_lines`]), sent at most
//!   every [`FLUSH`], at most [`MAX_FLUSH`] bytes at once, with a line that says how many were
//!   left out before them (`stream`). The time since it began is sent each second, and a
//!   remote's progress as it changes.
//! - A failure names the hooks that may have made it, from the hooks directory git says it
//!   uses (`core.hooksPath` respected).
//! - One request at a time; the UI refuses a second run while one is under way (ADR 0127,
//!   step 1b, Q8).

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use oso_core::editor::{CommitKind, CommitSwitches, FetchFrom, PullHow, PushSwitches};

use super::commit;
use super::discarded::{self, Entry};
use super::keep;
use super::read::{find, hooks_dir};
use super::remote::Push;
use crate::git::{Ender, Git, MAX_SHORT, TrustedRoot, os_path, trim_line};

mod remotes;
mod sequence;
mod stash;
mod stream;

pub(crate) use remotes::{Planned, Want};
pub(crate) use stash::StashRun;
use stream::stream;

/// How often what a run wrote is sent on.
pub(crate) const FLUSH: Duration = Duration::from_millis(100);
/// The most bytes sent on at once: a log shows the end of a flood, and says how much of it
/// was left out, so that a hook that writes fast costs the UI thread little.
pub(crate) const MAX_FLUSH: usize = 256 * 1024;

/// What the thread is asked to do.
pub(crate) enum RunRequest {
    /// Read `HEAD`'s message, which amending and rewording start from.
    Message,
    /// Read the last subjects, which a draft request names (step 6a).
    Subjects,
    /// Read what discards in this repository kept (step 1c).
    Discarded,
    /// Write the copy `entry` back to its file; `secret` if a buffer of the file may hold
    /// secrets, which no copy is kept of.
    Restore { entry: Entry, secret: bool },
    /// Make the commit `kind` with `switches`, `message` on standard input if it takes one, and
    /// the names `pass` passes on to its hooks; a fixup or a squash for the commit `target`.
    Commit {
        kind: CommitKind,
        switches: CommitSwitches,
        message: Option<Vec<u8>>,
        pass: Vec<String>,
        target: Option<String>,
    },
    /// A push's plan, or a push, a pull or a fetch (step 2).
    Remote(RemoteRun),
    /// Continue, skip or abort what is stopped, as `want` says, with the names `pass` passes on
    /// to the hooks (step 3a).
    Step {
        want: super::sequence::Want,
        pass: Vec<String>,
    },
    /// Push onto the stash, or apply, pop or drop a stash, as `run` says, with the names `pass`
    /// passes on to the hooks (step 3b).
    Stash { run: StashRun, pass: Vec<String> },
}

/// What a remote's request asks for, with the names `pass` passes on to the hooks of a run.
pub(crate) enum RemoteRun {
    /// Find where the push `want` asks for goes, with `switches`.
    Plan { want: Want, switches: PushSwitches },
    /// Run `push`, planned.
    Push { push: Push, pass: Vec<String> },
    /// Pull the upstream as `how` says.
    Pull { how: PullHow, pass: Vec<String> },
    /// Fetch from `from`, pruning.
    Fetch { from: FetchFrom, pass: Vec<String> },
}

/// How a run ended.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RunEnd {
    Done,
    /// It failed: what the echo area says, and the hooks that may have made it.
    Failed {
        said: String,
        hooks: Vec<&'static str>,
    },
    /// It was asked to stop, and did.
    Stopped,
    /// It did not start: why.
    Refused(String),
}

/// What the thread found.
pub(crate) enum RunReport {
    /// `HEAD`'s message, or why it could not be read.
    Message(Result<String, String>),
    /// The last subjects, newest first, none if they could not be read; and the request's id,
    /// or why there is none.
    Subjects {
        subjects: Vec<String>,
        id: Result<String, String>,
    },
    /// What discards in this repository kept, newest first, or why it could not be read.
    Discarded(Result<Vec<Entry>, String>),
    /// Where a push goes, or why it goes nowhere (step 2).
    Planned(Result<Planned, String>),
    /// The progress a remote's run shows now, as `Writing objects 42%`.
    Progress(String),
    /// A copy was written back: what the echo area says, or why not.
    Restored(Result<String, String>),
    /// A run began: the command line the log shows.
    Began { line: String },
    /// What it wrote, as the log shows it.
    Output(String),
    /// Whole seconds since it began.
    Elapsed(u64),
    /// It ended.
    Ended(RunEnd),
}

impl std::fmt::Debug for RunReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Message(_) => f.write_str("Message"),
            Self::Subjects { subjects, .. } => write!(f, "Subjects({})", subjects.len()),
            Self::Discarded(_) => f.write_str("Discarded"),
            Self::Planned(_) => f.write_str("Planned"),
            Self::Progress(_) => f.write_str("Progress"),
            Self::Restored(_) => f.write_str("Restored"),
            Self::Began { .. } => f.write_str("Began"),
            Self::Output(text) => write!(f, "Output({} bytes)", text.len()),
            Self::Elapsed(seconds) => write!(f, "Elapsed({seconds})"),
            Self::Ended(end) => write!(f, "Ended({end:?})"),
        }
    }
}

/// A repository's run thread. Dropping it stops a run under way.
pub(crate) struct Runner {
    requests: Sender<RunRequest>,
    stop: Arc<AtomicBool>,
    /// What ends the run under way, from the moment git starts: quitting reaches it before any
    /// report has.
    ender: Arc<Mutex<Option<Ender>>>,
}

/// What a run looks at as it goes: the stop the person asks for, the root's revocation, and
/// where its ender is put.
struct Watch<'a> {
    stop: &'a AtomicBool,
    root: &'a TrustedRoot,
    ender: &'a Mutex<Option<Ender>>,
}

impl Runner {
    /// Starts the thread for the project at `root`, with the hooks directory under `state` that
    /// the status's thread uses, handing each report to `send`, which says `false` once the
    /// event loop has gone.
    pub(crate) fn start(
        root: TrustedRoot,
        state: Option<PathBuf>,
        send: impl FnMut(RunReport) -> bool + Send + 'static,
    ) -> std::io::Result<Runner> {
        let (requests, queue) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let ender = Arc::new(Mutex::new(None));
        let (flag, slot) = (Arc::clone(&stop), Arc::clone(&ender));
        thread::Builder::new()
            .name("oso-vcs-run".into())
            .spawn(move || {
                let watch = Watch {
                    stop: &flag,
                    root: &root,
                    ender: &slot,
                };
                serve(&queue, send, &watch, state.as_deref());
            })?;
        Ok(Runner {
            requests,
            stop,
            ender,
        })
    }

    /// Sends the run under way's group `SIGTERM` at once, waiting for nothing: as oso ends.
    pub(crate) fn end(&self) {
        let ender = self.ender.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(ender) = ender.as_ref() {
            ender.end();
        }
    }

    /// Asks the thread for `request`; `false` if it has ended.
    pub(crate) fn ask(&self, request: RunRequest) -> bool {
        self.stop.store(false, Ordering::SeqCst);
        self.requests.send(request).is_ok()
    }

    /// Stops the run under way: `SIGTERM` to its group, `SIGKILL` after the grace.
    pub(crate) fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl Drop for Runner {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn serve(
    queue: &Receiver<RunRequest>,
    mut send: impl FnMut(RunReport) -> bool,
    watch: &Watch<'_>,
    state: Option<&Path>,
) {
    let root = watch.root;
    let mut git = None;
    while let Ok(request) = queue.recv() {
        let found = root.check().and_then(|()| match &git {
            Some(git) => Ok(Git::clone(git)),
            None => {
                let hooks = hooks_dir(state)?;
                let found = find(root.path(), &hooks)?;
                git = Some(found.clone());
                Ok(found)
            }
        });
        let reports = match (found, request) {
            (Err(why), RunRequest::Message) => vec![RunReport::Message(Err(why))],
            (found, RunRequest::Subjects) => vec![RunReport::Subjects {
                subjects: found.map(|git| subjects(&git)).unwrap_or_default(),
                id: super::agent::request_id(),
            }],
            (Err(why), RunRequest::Discarded) => vec![RunReport::Discarded(Err(why))],
            (Err(why), RunRequest::Restore { .. }) => vec![RunReport::Restored(Err(why))],
            (Ok(_), RunRequest::Discarded) => {
                let root = root.path().as_os_str().as_encoded_bytes();
                let read = state
                    .ok_or_else(|| "oso has no state directory".to_owned())
                    .and_then(|state| discarded::read(state).map_err(|e| e.to_string()))
                    .map(|entries| discarded::of_root(entries, root));
                vec![RunReport::Discarded(read)]
            }
            (Ok(git), RunRequest::Restore { entry, secret }) => {
                let done = keep::restore(&git, root, (&entry, secret), state, watch.stop);
                vec![RunReport::Restored(done)]
            }
            (Err(why), RunRequest::Remote(RemoteRun::Plan { .. })) => {
                vec![RunReport::Planned(Err(why))]
            }
            // Every other run: it did not start.
            (Err(why), _) => vec![RunReport::Ended(RunEnd::Refused(why))],
            (Ok(git), request @ (RunRequest::Step { .. } | RunRequest::Stash { .. })) => {
                vec![local(&git, request, watch, &mut send)]
            }
            (Ok(git), RunRequest::Remote(request)) => {
                vec![remote(&git, request, watch, &mut send)]
            }
            (Ok(git), RunRequest::Message) => vec![RunReport::Message(message(&git))],
            (
                Ok(git),
                RunRequest::Commit {
                    kind,
                    switches,
                    message,
                    pass,
                    target,
                },
            ) => {
                let run = Commit {
                    git: git.with_own_hooks(&pass),
                    kind,
                    switches,
                    message,
                    target,
                };
                let end = run.run(watch, &mut send);
                vec![ended(watch, end)]
            }
        };
        for report in reports {
            if !send(report) {
                return;
            }
        }
    }
}

/// What a remote's `request` comes to: a push's plan, or how its run ended.
fn remote(
    git: &Git,
    request: RemoteRun,
    watch: &Watch<'_>,
    send: &mut impl FnMut(RunReport) -> bool,
) -> RunReport {
    let end = match request {
        RemoteRun::Plan { want, switches } => {
            return RunReport::Planned(remotes::plan(git, want, switches));
        }
        RemoteRun::Push { push, pass } => remotes::push(git, &push, &pass, watch, send),
        RemoteRun::Pull { how, pass } => remotes::pull(git, how, &pass, watch, send),
        RemoteRun::Fetch { from, pass } => remotes::fetch(git, from, &pass, watch, send),
    };
    ended(watch, end)
}

/// How a run that touches no remote ended: a step of what is stopped, or a stash's.
fn local(
    git: &Git,
    request: RunRequest,
    watch: &Watch<'_>,
    send: &mut impl FnMut(RunReport) -> bool,
) -> RunReport {
    let end = match request {
        RunRequest::Step { want, pass } => sequence::step(git, &want, &pass, watch, send),
        RunRequest::Stash { run, pass } => stash::stash(git, &run, &pass, watch, send),
        _ => RunEnd::Refused("not a run of this kind".into()),
    };
    ended(watch, end)
}

/// The report of a run that ended as `end`, once its ender is let go.
fn ended(watch: &Watch<'_>, end: RunEnd) -> RunReport {
    *watch.ender.lock().unwrap_or_else(PoisonError::into_inner) = None;
    RunReport::Ended(end)
}

/// `HEAD`'s message, as git keeps it.
fn message(git: &Git) -> Result<String, String> {
    let words = [
        "log",
        "-1",
        "--no-show-signature",
        "--format=%B",
        "HEAD",
        "--",
    ];
    let words: Vec<&OsStr> = words.iter().map(OsStr::new).collect();
    let out = git
        .run(&words, &[], commit::MAX_MESSAGE)
        .map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&out).into_owned())
}

/// The last subjects, newest first, each as git gives it, lossily: none before the first
/// commit, or if git fails.
fn subjects(git: &Git) -> Vec<String> {
    let count = format!("-{}", super::agent::SUBJECTS);
    let words = [
        "log",
        &count,
        "--no-show-signature",
        "--format=%s",
        "HEAD",
        "--",
    ];
    let words: Vec<&OsStr> = words.iter().map(OsStr::new).collect();
    let Ok(out) = git.run(&words, &[], MAX_SUBJECTS) else {
        return Vec::new();
    };
    (String::from_utf8_lossy(&out).lines())
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The most bytes of subjects read.
const MAX_SUBJECTS: usize = 64 * 1024;

/// A commit to run.
struct Commit {
    git: Git,
    kind: CommitKind,
    switches: CommitSwitches,
    message: Option<Vec<u8>>,
    target: Option<String>,
}

impl Commit {
    /// Runs it to its end, a stop or the root's revocation, sending what it writes; `false`
    /// from `send` ends it.
    fn run(self, watch: &Watch<'_>, send: &mut impl FnMut(RunReport) -> bool) -> RunEnd {
        let words = commit::args(self.kind, self.switches, self.target.as_deref());
        let input = self.message.filter(|_| commit::takes_message(self.kind));
        let streamed = match stream(&self.git, &words, input, watch, send, false) {
            Ok(streamed) => streamed,
            Err(why) => return RunEnd::Refused(why),
        };
        match streamed.status {
            _ if streamed.stopped => RunEnd::Stopped,
            Ok(status) if status.success() => RunEnd::Done,
            Ok(_) => {
                let hooks = hooks_of(&self.git)
                    .map(|dir| commit::hooks_present(&dir, self.switches.no_verify))
                    .unwrap_or_default();
                RunEnd::Failed {
                    said: commit::failed(&hooks),
                    hooks,
                }
            }
            Err(why) => RunEnd::Failed {
                said: format!("commit failed: {why}"),
                hooks: Vec::new(),
            },
        }
    }
}

/// The hooks directory git uses for this repository, `core.hooksPath` respected.
fn hooks_of(git: &Git) -> Option<PathBuf> {
    let words = ["rev-parse", "--git-path", "hooks"].map(OsStr::new);
    let out = git.run(&words, &[], MAX_SHORT).ok()?;
    let path = os_path(trim_line(&out)).ok()?;
    let path = Path::new(path);
    Some(if path.is_absolute() {
        path.to_path_buf()
    } else {
        git.top().join(path)
    })
}

#[cfg(test)]
mod tests;
