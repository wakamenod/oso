//! A status buffer's thread, `oso-vcs` (ADR 0127): it reads the status, the open files' hunks
//! and the head, runs what `s` and `u` asked, and lays out the buffer's text.
//!
//! Must not depend on: `app`, `dispatch`, winit or the UI thread; the caller hands requests in,
//! takes reports out, and supplies the wake-up.
//!
//! - It starts only from a [`TrustedRoot`], and looks at the root again before each batch: a
//!   restricted project, or another directory put in a trusted one's place, runs no git (ADR
//!   0009, ADR 0011, ADR 0114). The repository's top must be the root, as a review's.
//! - git runs through `git`'s runner: oso's arguments and environment, no hook and no file
//!   system monitor, each read bounded, a read past ten seconds killed. A write to the index
//!   has no time limit, as a kill could leave the index locked (ADR 0127, step 1b): `C-g`
//!   stops it, with `SIGTERM` first, which git cleans up on. Every run here adds
//!   `GIT_LITERAL_PATHSPECS=1`, so that a path is only ever itself, and paths go to `add` and
//!   `restore` on standard input. No hook runs: every index write would run
//!   `post-index-change`, and only the runner's empty hooks directory keeps it from running.
//! - The root is looked at before each batch and each write, and its revocation before each
//!   read, so that a revoked project runs nothing more.
//! - Reads ask for git's own bytes and keep the person's configuration from reshaping them:
//!   diffs run with `--no-ext-diff --no-textconv --no-color --no-relative` and `a/` and `b/`
//!   prefixes, and a hunk is applied with no whitespace loosening, so that what was shown is
//!   what goes in, or git refuses it.
//! - What waits is taken as one batch: each staging in order, then one reading and one text.
//! - Nothing here logs a file's text; a report's `Debug` gives its kind.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use super::layout::Built;
use super::layout::{self, Head, Hunks, Laid, Section, View};
use super::read::{MAX_LIST, args, find, head_commit, hooks_dir, numstat, read_patch, set};
use super::stage::{self, Op, Target};
use super::status::{self, Status};
use super::write::run;
use crate::git::{Git, GitError};
use crate::git::{TrustedRoot, may_hold_secrets};

/// What the thread is asked to do. Each carries the view to lay out with.
pub(crate) enum Request {
    /// Read the status again, then lay it out.
    Read { generation: u64, view: View },
    /// Lay out what was read, reading the hunks of files newly open.
    Layout { generation: u64, view: View },
    /// Stage (`stage`) or unstage `targets`, which name what `built` shows, then read the status
    /// again. The git commands are made here, off the UI thread: a section's may name every
    /// path of a large repository.
    Stage {
        generation: u64,
        built: Arc<Built>,
        targets: Vec<Target>,
        stage: bool,
        view: View,
    },
}

impl Request {
    fn generation(&self) -> u64 {
        match self {
            Self::Read { generation, .. }
            | Self::Layout { generation, .. }
            | Self::Stage { generation, .. } => *generation,
        }
    }

    fn view(&self) -> &View {
        match self {
            Self::Read { view, .. } | Self::Layout { view, .. } | Self::Stage { view, .. } => view,
        }
    }
}

/// What the thread found.
pub(crate) enum Report {
    /// A text, laid out for the newest request of `generation` and those before it.
    Laid { generation: u64, laid: Box<Laid> },
    /// Staging ran; what the echo area says.
    Ran { said: String },
    /// Staging was refused before git ran: why, for the echo area.
    Refused { why: String },
    /// Something failed: why, with no file's text.
    Failed { why: String },
}

impl fmt::Debug for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Laid { generation, .. } => f
                .debug_struct("Laid")
                .field("generation", generation)
                .finish_non_exhaustive(),
            Self::Ran { said } => f.debug_struct("Ran").field("said", said).finish(),
            Self::Refused { why } => f.debug_struct("Refused").field("why", why).finish(),
            Self::Failed { why } => f.debug_struct("Failed").field("why", why).finish(),
        }
    }
}

/// A status buffer's thread. Dropping it ends the thread once its batch is done, and stops a
/// write under way.
pub(crate) struct Worker {
    requests: Sender<Request>,
    live: Arc<AtomicBool>,
    /// Set to stop the write under way; the thread clears it as its next batch begins.
    stop: Arc<AtomicBool>,
}

impl Worker {
    /// Starts the thread for the project at `root`, telling git that an empty private
    /// directory under `state`, oso's own in the per-user state directory, holds the hooks, and
    /// handing each report to `send`, which says `false` once the event loop has gone.
    pub(crate) fn start(
        root: TrustedRoot,
        state: Option<PathBuf>,
        send: impl FnMut(Report) -> bool + Send + 'static,
    ) -> std::io::Result<Worker> {
        let (requests, queue) = mpsc::channel::<Request>();
        let live = Arc::new(AtomicBool::new(true));
        let stop = Arc::new(AtomicBool::new(false));
        let flags = (Arc::clone(&live), Arc::clone(&stop));
        thread::Builder::new()
            .name("oso-vcs".into())
            .spawn(move || serve(&queue, send, &root, state, &flags))?;
        Ok(Worker {
            requests,
            live,
            stop,
        })
    }

    /// Asks the thread for `request`; `false` if it has ended.
    pub(crate) fn ask(&self, request: Request) -> bool {
        self.requests.send(request).is_ok()
    }

    /// Stops the write to the index under way, and those waiting in its batch.
    pub(crate) fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.live.store(false, Ordering::SeqCst);
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// Answers each batch until the worker is dropped or the event loop has gone.
fn serve(
    queue: &Receiver<Request>,
    mut send: impl FnMut(Report) -> bool,
    root: &TrustedRoot,
    state_dir: Option<PathBuf>,
    (live, stop): &(Arc<AtomicBool>, Arc<AtomicBool>),
) {
    let mut state = State {
        state_dir: state_dir.clone(),
        ..State::default()
    };
    while let Ok(first) = queue.recv() {
        let mut batch = vec![first];
        batch.extend(queue.try_iter());
        if !live.load(Ordering::SeqCst) {
            return;
        }
        if let Err(why) = root.check() {
            if !send(Report::Failed { why }) {
                return;
            }
            continue;
        }
        let mut reports = Vec::new();
        // A stop is for the batch it came in, and not one asked for after it.
        stop.store(false, Ordering::SeqCst);
        state.stop = Arc::clone(stop);
        let answered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            state.answer(batch, root, &mut reports);
        }));
        if answered.is_err() {
            log::warn!("a git status's batch panicked");
            state = State {
                state_dir: state_dir.clone(),
                ..State::default()
            };
            reports.push(Report::Failed {
                why: "the status's thread failed".into(),
            });
        }
        for report in reports {
            if !send(report) {
                return;
            }
        }
    }
}

/// What the thread keeps between batches.
#[derive(Default)]
struct State {
    /// oso's own directory in the per-user state directory.
    state_dir: Option<PathBuf>,
    git: Option<Git>,
    head: Head,
    notes: Vec<String>,
    status: Status,
    unstaged: HashMap<Vec<u8>, status::Counts>,
    staged: HashMap<Vec<u8>, status::Counts>,
    /// The open files' hunks read since the status was.
    hunks: HashMap<(Section, Vec<u8>), Hunks>,
    /// Whether the status was read at all.
    read: bool,
    /// Set to stop the write under way.
    stop: Arc<AtomicBool>,
}

impl State {
    fn answer(&mut self, batch: Vec<Request>, trusted: &TrustedRoot, out: &mut Vec<Report>) {
        let Some(last) = batch.last() else {
            return;
        };
        let root = trusted.path();
        let generation = batch.iter().map(Request::generation).max().unwrap_or(0);
        let view = last.view().clone();
        let mut read = !self.read;
        for request in batch {
            match request {
                Request::Read { .. } => read = true,
                Request::Layout { .. } => {}
                Request::Stage {
                    built,
                    targets,
                    stage,
                    ..
                } => match stage::ops(&built, &targets, stage) {
                    Ok((ops, said)) => {
                        read = true;
                        match self.run_all(&ops, trusted) {
                            Ok(()) => out.push(Report::Ran { said }),
                            Err(why) => out.push(Report::Failed { why }),
                        }
                    }
                    Err(why) => out.push(Report::Refused { why }),
                },
            }
        }
        if read && let Err(why) = self.read_status(trusted) {
            out.push(Report::Failed { why: why.clone() });
            self.notes = vec![format!("git status failed: {why}")];
        }
        // Once a batch: a file may hold secrets by its name, or as an open buffer of it does.
        let secret = |path: &[u8]| may_hold_secrets(root, path) || view.withheld.contains(path);
        let sections = layout::sections(&self.status, &self.unstaged, &self.staged, &secret);
        self.read_open_hunks(&view, &sections, trusted);
        let laid = layout::lay_out(&self.head, &self.notes, sections, &self.hunks, &view);
        out.push(Report::Laid {
            generation,
            laid: Box::new(laid),
        });
    }

    /// git in the repository whose top is `root`, found once, with the empty private
    /// directory for hooks made under the state directory.
    fn git(&mut self, root: &Path) -> Result<&Git, String> {
        if self.git.is_none() {
            let hooks = hooks_dir(self.state_dir.as_deref())?;
            self.git = Some(find(root, &hooks)?);
        }
        self.git.as_ref().ok_or_else(|| "no git".to_owned())
    }

    fn read_status(&mut self, trusted: &TrustedRoot) -> Result<(), String> {
        self.read = true;
        self.hunks.clear();
        self.notes.clear();
        let git = self.git(trusted.path())?.clone();
        let still = || {
            (!trusted.revoked())
                .then_some(())
                .ok_or_else(|| "the project is no longer trusted".to_owned())
        };
        let listing = git
            .output(
                &args(&[
                    "status",
                    "--porcelain=v2",
                    "-z",
                    "--branch",
                    "--untracked-files=all",
                ]),
                &set(),
                MAX_LIST,
            )
            .map_err(|e| e.to_string())?;
        if !listing.status.success() {
            return Err(GitError::Failed {
                status: listing.status.code(),
                stderr: listing.stderr,
            }
            .to_string());
        }
        let mut listed = listing.stdout;
        if listing.cut {
            self.notes
                .push("git listed more than oso reads: some files are not shown".into());
            // The last record may stop part way: it is left out.
            let whole = listed
                .iter()
                .rposition(|&byte| byte == 0)
                .map_or(0, |at| at + 1);
            listed.truncate(whole);
        }
        self.status = status::parse(&listed);
        still()?;
        let diff = [
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--numstat",
            "-z",
        ];
        self.unstaged = numstat(&git, &args(&diff)).unwrap_or_default();
        let mut cached = args(&diff);
        cached.push(OsStr::new("--cached"));
        self.staged = numstat(&git, &cached).unwrap_or_default();
        still()?;
        self.head = Head {
            branch: self.status.branch.clone(),
            commit: self
                .status
                .branch
                .oid
                .as_ref()
                .and_then(|_| head_commit(&git)),
        };
        Ok(())
    }

    /// Reads the hunks of each open file of `sections` not read since the status was, while
    /// the patches kept stay within what a status shows.
    fn read_open_hunks(
        &mut self,
        view: &View,
        sections: &[(Section, Vec<layout::File>)],
        root: &TrustedRoot,
    ) {
        let Some(git) = self.git.clone() else {
            return;
        };
        let mut kept: usize = (self.hunks.values())
            .map(|hunks| match hunks {
                Hunks::Patch(patch) => patch.len(),
                Hunks::Note(_) | Hunks::Whole(_) => 0,
            })
            .sum();
        for (section, files) in sections {
            for file in files.iter().filter(|file| file.reads) {
                let key = (*section, file.path.clone());
                if !view.is_open(*section, &file.path) || self.hunks.contains_key(&key) {
                    continue;
                }
                let hunks = if root.revoked() {
                    Hunks::Note("not read: the project is no longer trusted".into())
                } else if kept >= layout::MAX_TEXT {
                    Hunks::Note("not read: the status holds as much as oso shows".into())
                } else {
                    match read_patch(&git, *section, &file.path, file.from.as_deref()) {
                        Ok(patch) => {
                            kept += patch.len();
                            Hunks::Patch(Arc::new(patch))
                        }
                        Err(why) => Hunks::Note(format!("cannot read its hunks: {why}")),
                    }
                };
                self.hunks.insert(key, hunks);
            }
        }
    }

    /// Runs `ops` in order, each only while the root is still the one trusted.
    fn run_all(&mut self, ops: &[Op], root: &TrustedRoot) -> Result<(), String> {
        let initial = self.read && self.status.branch.oid.is_none();
        let git = self.git(root.path())?.clone();
        for op in ops {
            root.check()?;
            run(&git, op, initial, &self.stop)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
