//! A repository's thread for git's long runs, `oso-vcs-run` (ADR 0127, step 1b): a commit with
//! the repository's hooks, which has no time limit, and the message an amend starts from.
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
//!   left out before them. The time since it began is sent each second.
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
use std::time::{Duration, Instant};

use oso_core::editor::{CommitKind, CommitSwitches};

use super::commit;
use super::work::{find, hooks_dir};
use crate::git::{Ender, Git, MAX_SHORT, Next, TrustedRoot, os_path, trim_line};

/// How often what a run wrote is sent on.
pub(crate) const FLUSH: Duration = Duration::from_millis(100);
/// The most bytes sent on at once: a log shows the end of a flood, and says how much of it
/// was left out, so that a hook that writes fast costs the UI thread little.
pub(crate) const MAX_FLUSH: usize = 256 * 1024;

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
                let end = run.run(watch, &mut send);
                *watch.ender.lock().unwrap_or_else(PoisonError::into_inner) = None;
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
    /// Runs it to its end, a stop or the root's revocation, sending what it writes; `false`
    /// from `send` ends it.
    fn run(self, watch: &Watch<'_>, send: &mut impl FnMut(RunReport) -> bool) -> RunEnd {
        let words = commit::args(self.kind, self.switches);
        let args: Vec<&OsStr> = words.iter().map(OsStr::new).collect();
        let input = self.message.filter(|_| commit::takes_message(self.kind));
        let mut long = match self.git.start_long(&args, &[], input) {
            Ok(long) => long,
            Err(e) => return RunEnd::Refused(e.to_string()),
        };
        *watch.ender.lock().unwrap_or_else(PoisonError::into_inner) = Some(long.ender());
        let line = format!("$ git {}\n", words.join(" "));
        if !send(RunReport::Began { line }) {
            return RunEnd::Stopped;
        }
        let began = Instant::now();
        let mut flushed = Instant::now();
        let mut seconds = 0;
        let (mut partial, mut pending) = (Vec::new(), Waiting::default());
        let status = loop {
            // A project no longer trusted runs nothing more, whatever the UI thread does.
            if watch.stop.load(Ordering::SeqCst) || watch.root.revoked() {
                long.stop();
            }
            match long.next(FLUSH) {
                Ok(Next::Out(piece)) => pending.push(&commit::log_lines(&piece, &mut partial)),
                Ok(Next::Quiet) => {}
                Ok(Next::Ended(status)) => break Ok(status),
                Err(e) => break Err(e.to_string()),
            }
            if flushed.elapsed() >= FLUSH
                && let Some(text) = pending.take()
            {
                flushed = Instant::now();
                if !send(RunReport::Output(text)) {
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
        pending.push(&commit::log_rest(&mut partial));
        if let Some(text) = pending.take() {
            send(RunReport::Output(text));
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

/// What a run wrote that waits to be sent on, and how many bytes were left out before it.
#[derive(Default)]
struct Waiting {
    text: String,
    left_out: usize,
}

impl Waiting {
    /// Adds whole `lines`; past twice [`MAX_FLUSH`], only the last of them stay.
    fn push(&mut self, lines: &str) {
        self.text.push_str(lines);
        if self.text.len() > 2 * MAX_FLUSH {
            self.left_out += keep_tail(&mut self.text, MAX_FLUSH);
        }
    }

    /// What to send: at most [`MAX_FLUSH`] bytes, after a line that says how many were left
    /// out, if any; `None` with nothing to say.
    fn take(&mut self) -> Option<String> {
        self.left_out += keep_tail(&mut self.text, MAX_FLUSH);
        if self.text.is_empty() && self.left_out == 0 {
            return None;
        }
        let mut out = match std::mem::take(&mut self.left_out) {
            0 => String::new(),
            n => format!("[… {n} bytes left out]\n"),
        };
        out.push_str(&std::mem::take(&mut self.text));
        Some(out)
    }
}

/// Keeps the end of `text`, at most `keep` bytes from a line's start, and says how many went.
fn keep_tail(text: &mut String, keep: usize) -> usize {
    if text.len() <= keep {
        return 0;
    }
    let from = text.ceil_char_boundary(text.len() - keep);
    let from = text[from..].find('\n').map_or(from, |at| from + at + 1);
    text.drain(..from);
    from
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
