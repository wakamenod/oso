//! A repository's thread for git's long runs, `oso-vcs-run` (ADR 0127, step 1b): a commit with
//! the repository's hooks, which has no time limit, and the message an amend starts from.
//!
//! Must not depend on: `app`, `dispatch`, winit or the UI thread; the caller hands requests in,
//! takes reports out, and supplies the wake-up.
//!
//! - It starts only from a [`TrustedRoot`], looks at the root before each request, and finds
//!   git as the status's thread does: the repository's top must be the root.
//! - A commit runs the repository's own hooks, with the environment `git`'s runner gives a
//!   hook and the names of `[git] pass_env`, in a process group of its own, so that a stop
//!   ends what a hook started too. No `GIT_LITERAL_PATHSPECS`: a hook's own git takes its
//!   paths as patterns.
//! - What git writes streams out as lines a log shows ([`commit::log_lines`]), sent at most
//!   every [`FLUSH`], and never more than the log keeps at once. The time since it began is
//!   sent each second.
//! - A failure names the hooks that may have made it, from the hooks directory git says it
//!   uses (`core.hooksPath` respected).
//! - One request at a time; the UI refuses a second run while one is under way (ADR 0127,
//!   step 1b, Q8).

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use oso_core::editor::{CommitKind, CommitSwitches, MAX_GIT_LOG};

use super::commit;
use super::work::{find, hooks_dir};
use crate::git::{Ender, Git, MAX_SHORT, Next, TrustedRoot, os_path, trim_line};

/// How often what a run wrote is sent on.
pub(crate) const FLUSH: Duration = Duration::from_millis(100);

/// What the thread is asked to do.
pub(crate) enum RunRequest {
    /// Read `HEAD`'s message, which amending and rewording start from.
    Message,
    /// Make the commit `kind` with `switches`, `message` on standard input if it takes one, and
    /// the names `pass` passes on to its hooks.
    Commit {
        kind: CommitKind,
        switches: CommitSwitches,
        message: Option<Vec<u8>>,
        pass: Vec<String>,
    },
}

/// How a run ended.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RunEnd {
    Done,
    /// It failed: what the echo area says.
    Failed(String),
    /// It was asked to stop, and did.
    Stopped,
    /// It did not start: why.
    Refused(String),
}

/// What the thread found.
pub(crate) enum RunReport {
    /// `HEAD`'s message, or why it could not be read.
    Message(Result<String, String>),
    /// A run began: the command line the log shows, and what ends it from elsewhere.
    Began { line: String, ender: Ender },
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
        let flag = Arc::clone(&stop);
        thread::Builder::new()
            .name("oso-vcs-run".into())
            .spawn(move || serve(&queue, send, &root, state.as_deref(), &flag))?;
        Ok(Runner { requests, stop })
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
    root: &TrustedRoot,
    state: Option<&Path>,
    stop: &AtomicBool,
) {
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
            (Err(why), RunRequest::Commit { .. }) => vec![RunReport::Ended(RunEnd::Refused(why))],
            (Ok(git), RunRequest::Message) => vec![RunReport::Message(message(&git))],
            (
                Ok(git),
                RunRequest::Commit {
                    kind,
                    switches,
                    message,
                    pass,
                },
            ) => {
                let run = Commit {
                    git: git.with_own_hooks(&pass),
                    kind,
                    switches,
                    message,
                };
                let end = run.run(stop, &mut send);
                vec![RunReport::Ended(end)]
            }
        };
        for report in reports {
            if !send(report) {
                return;
            }
        }
    }
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

/// A commit to run.
struct Commit {
    git: Git,
    kind: CommitKind,
    switches: CommitSwitches,
    message: Option<Vec<u8>>,
}

impl Commit {
    /// Runs it to its end or a stop, sending what it writes; `false` from `send` ends it.
    fn run(self, stop: &AtomicBool, send: &mut impl FnMut(RunReport) -> bool) -> RunEnd {
        let words = commit::args(self.kind, self.switches);
        let args: Vec<&OsStr> = words.iter().map(OsStr::new).collect();
        let input = self.message.filter(|_| commit::takes_message(self.kind));
        let mut long = match self.git.start_long(&args, &[], input) {
            Ok(long) => long,
            Err(e) => return RunEnd::Refused(e.to_string()),
        };
        let line = format!("$ git {}\n", words.join(" "));
        if !send(RunReport::Began {
            line,
            ender: long.ender(),
        }) {
            return RunEnd::Stopped;
        }
        let began = Instant::now();
        let mut flushed = Instant::now();
        let mut seconds = 0;
        let (mut partial, mut pending) = (Vec::new(), String::new());
        let status = loop {
            if stop.load(Ordering::SeqCst) {
                long.stop();
            }
            match long.next(FLUSH) {
                Ok(Next::Out(piece)) => {
                    pending.push_str(&commit::log_lines(&piece, &mut partial));
                    keep_tail(&mut pending);
                }
                Ok(Next::Quiet) => {}
                Ok(Next::Ended(status)) => break Ok(status),
                Err(e) => break Err(e.to_string()),
            }
            if flushed.elapsed() >= FLUSH && !pending.is_empty() {
                flushed = Instant::now();
                if !send(RunReport::Output(std::mem::take(&mut pending))) {
                    long.stop();
                }
            }
            let now = began.elapsed().as_secs();
            if now > seconds {
                seconds = now;
                if !send(RunReport::Elapsed(now)) {
                    long.stop();
                }
            }
        };
        pending.push_str(&commit::log_rest(&mut partial));
        if !pending.is_empty() {
            send(RunReport::Output(pending));
        }
        match status {
            _ if long.stopped() => RunEnd::Stopped,
            Ok(status) if status.success() => RunEnd::Done,
            Ok(_) => {
                let hooks = hooks_of(&self.git)
                    .map(|dir| commit::hooks_present(&dir, self.switches.no_verify))
                    .unwrap_or_default();
                RunEnd::Failed(commit::failed(&hooks))
            }
            Err(why) => RunEnd::Failed(format!("commit failed: {why}")),
        }
    }
}

/// Keeps the end of `text`, at most what a log keeps, from a line's start.
fn keep_tail(text: &mut String) {
    if text.len() <= MAX_GIT_LOG {
        return;
    }
    let from = text.ceil_char_boundary(text.len() - MAX_GIT_LOG);
    let from = text[from..].find('\n').map_or(from, |at| from + at + 1);
    text.drain(..from);
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
