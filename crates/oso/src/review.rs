//! A review's git and diffing (ADR 0122): one thread per review, `oso-review`, that runs its
//! git commands in order, reads each file's sides, diffs them with `similar` and hands each
//! file's reading to the event loop.
//!
//! Must not depend on: `app`, `dispatch`, winit or the UI thread; the caller hands requests in
//! and takes reports out, and supplies the wake-up.
//!
//! - It starts only from a [`TrustedRoot`], which only a trusted project gives, and looks at
//!   the root's device and inode again before each request: a restricted project, or another
//!   directory put in a trusted one's place, runs no git (ADR 0009, ADR 0011, ADR 0114). A
//!   root revoked when its project is no longer trusted runs nothing more, and stops the
//!   request running between files.
//! - git runs only through `oso-platform::process`, with oso's arguments, never finding a
//!   repository above the root, and its output is read as bytes, each read bounded.
//! - Old sides, and new sides that are no work tree file, come through one long-lived
//!   `git cat-file --batch`; work tree files are read from the work tree's top a directory at
//!   a time, following no link.
//! - A file whose name, or whose name before a rename, may hold secrets is listed and never
//!   read, nor is a work tree file with more than one name. A conflicted file is listed as
//!   conflicted, its sides not read.
//! - A file whose signature did not change since the last reading is not read again, and the
//!   files the caller shows go first. Refreshes asked for meanwhile are answered as one, and a
//!   newer request, or the reviewer's end, stops the one running between files.
//! - Limits, each saying what it hides: 10,000 files and 32 MiB of list; a side of 8 MiB and
//!   200,000 lines, diffed within 1 s, and 5 s of diffing per request; 64 MiB of sides and
//!   hunks held per review; a git run of 10 s.
//! - The thread's stack is [`STACK`]: Myers' diff recurses, and needs up to half a MiB at the
//!   line limit.
//! - Nothing here logs a file's text; `Debug` of a reading gives its path and kind.

mod batch;
pub(crate) mod count;
mod diff;
pub(crate) mod git;
mod id;
mod index;
mod list;
mod read;
mod scope;
mod side;
#[cfg(test)]
pub(crate) mod tests;
pub(crate) mod view;
mod work;

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use oso_core::editor::Editor;
use oso_core::project::ProjectId;
use oso_core::review::{Carried, Carry, Shape};

pub(crate) use id::Id;
pub(crate) use list::Entry;
#[cfg(test)]
pub(crate) use list::Status;
pub(crate) use scope::Scope;

/// The largest side read: a blob or a work tree file past it shows its size only.
pub(crate) const MAX_BLOB: u64 = 8 * 1024 * 1024;
/// The review thread's stack.
const STACK: usize = 16 * 1024 * 1024;

/// The root of a project that is trusted to run git, with its device and inode as the project
/// was found. Only [`TrustedRoot::of`] makes one. Its clones share one revocation: once one is
/// revoked, so are all.
#[derive(Clone, Debug)]
pub(crate) struct TrustedRoot {
    path: PathBuf,
    identity: Option<(u64, u64)>,
    revoked: Arc<AtomicBool>,
}

impl TrustedRoot {
    /// The root of project `id`, if the editor trusts it.
    pub(crate) fn of(editor: &Editor, id: ProjectId) -> Option<TrustedRoot> {
        if !editor.project_trusted(id) {
            return None;
        }
        let project = editor.project(id)?;
        Some(TrustedRoot {
            path: project.root().to_owned(),
            identity: project.identity(),
            revoked: Arc::default(),
        })
    }

    #[cfg(test)]
    pub(crate) fn for_tests(path: PathBuf) -> TrustedRoot {
        let identity = side::identity_of(&path);
        TrustedRoot {
            path,
            identity,
            revoked: Arc::default(),
        }
    }

    /// The same trust for `path`, a linked worktree a worktree job just made in this root's
    /// repository, which belongs with it as ADR 0114 has it (ADR 0121). Its revocation is this
    /// root's. It looks at the disk, so the job calls it.
    pub(crate) fn for_worktree(&self, path: PathBuf) -> TrustedRoot {
        let identity = side::identity_of(&path);
        TrustedRoot {
            path,
            identity,
            revoked: Arc::clone(&self.revoked),
        }
    }

    /// Takes the trust back, as when the person no longer trusts the project: the review's
    /// thread runs nothing more for it.
    pub(crate) fn revoke(&self) {
        self.revoked.store(true, Ordering::SeqCst);
    }

    /// Whether the trust was taken back.
    pub(crate) fn revoked(&self) -> bool {
        self.revoked.load(Ordering::SeqCst)
    }

    /// Whether git may still run for the root: it is not revoked, and is still the directory
    /// that was trusted. It looks at the disk, so it belongs off the UI thread.
    ///
    /// # Errors
    ///
    /// Why not, for a person.
    pub(crate) fn check(&self) -> Result<(), String> {
        if self.revoked() {
            return Err("the project is no longer trusted".into());
        }
        if !self.still_there() {
            return Err("the project's root is no longer the directory that was trusted".into());
        }
        Ok(())
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Whether the root is still the directory that was trusted, by its device and inode where
    /// the system has them, not following a link.
    fn still_there(&self) -> bool {
        match self.identity {
            Some(identity) => side::identity_of(&self.path) == Some(identity),
            None => std::fs::symlink_metadata(&self.path).is_ok_and(|metadata| metadata.is_dir()),
        }
    }
}

/// What the review's thread is asked to do.
pub(crate) enum Request {
    /// Review `scope`: list its files and read each.
    Open(Scope),
    /// List again and read the files whose signature changed, those in `first` before the
    /// rest.
    Refresh { first: Vec<Box<[u8]>> },
    /// Take a baseline of the work tree for the session scope.
    Baseline,
    /// Carry comments to a new reading (`oso_core::review::Review::carry`).
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "comments are carried here from ADR 0122's step 3b"
        )
    )]
    Carry(Carry),
}

/// Which request a failure answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Asked {
    Open,
    Refresh,
    Baseline,
    Carry,
}

/// What the review's thread found. Each listing, the readings after it and the `Read` that
/// ends them share a generation.
pub(crate) enum Report {
    /// The files the scope differs in, in git's order, and how many more were left out; `cut`
    /// if git wrote more than a review reads.
    Listed {
        generation: u64,
        entries: Arc<[Entry]>,
        more: usize,
        cut: bool,
    },
    /// One file's reading, new or changed since the last one.
    File {
        generation: u64,
        reading: Arc<Reading>,
    },
    /// Every file of this generation that changed has been reported, unless `stopped` by a
    /// newer request, which reads the rest.
    Read { generation: u64, stopped: bool },
    /// A baseline was taken; at least `left_out` files could not be read.
    Baseline { tree: Id, left_out: usize },
    /// The session's baseline is gone.
    BaselineGone,
    /// Comments carried; `cut_short` if matching lines ran out of time, so some may be outdated
    /// that would not have been.
    Carried {
        #[cfg_attr(
            not(test),
            expect(
                dead_code,
                reason = "comments are carried here from ADR 0122's step 3b"
            )
        )]
        carried: Carried,
        cut_short: bool,
    },
    /// A request failed; the message says why, with no file's text.
    Failed { asked: Asked, why: String },
}

impl fmt::Debug for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Listed {
                generation,
                entries,
                more,
                cut,
            } => f
                .debug_struct("Listed")
                .field("generation", generation)
                .field("entries", &entries.len())
                .field("more", more)
                .field("cut", cut)
                .finish(),
            Self::File {
                generation,
                reading,
            } => f
                .debug_struct("File")
                .field("generation", generation)
                .field("reading", reading)
                .finish(),
            Self::Read {
                generation,
                stopped,
            } => f
                .debug_struct("Read")
                .field("generation", generation)
                .field("stopped", stopped)
                .finish(),
            Self::Baseline { tree, left_out } => f
                .debug_struct("Baseline")
                .field("tree", tree)
                .field("left_out", left_out)
                .finish(),
            Self::BaselineGone => f.write_str("BaselineGone"),
            Self::Carried { cut_short, .. } => f
                .debug_struct("Carried")
                .field("cut_short", cut_short)
                .finish_non_exhaustive(),
            Self::Failed { asked, why } => f
                .debug_struct("Failed")
                .field("asked", asked)
                .field("why", why)
                .finish(),
        }
    }
}

/// One reading of a file.
pub(crate) struct Reading {
    pub(crate) entry: Entry,
    pub(crate) content: Content,
}

impl Reading {
    /// The lines added and removed, as its hunks count them, for a text reading.
    pub(crate) fn counts(&self) -> Option<(usize, usize)> {
        let Content::Text { shape, .. } = &self.content else {
            return None;
        };
        Some(shape.hunks().iter().fold((0, 0), |(added, removed), hunk| {
            (added + hunk.added.len(), removed + hunk.removed.len())
        }))
    }
}

impl fmt::Debug for Reading {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match &self.content {
            Content::Text { shape, .. } => format!("text, {} hunks", shape.hunks().len()),
            Content::Binary { .. } => "binary".into(),
            Content::TooLarge { .. } => "too large".into(),
            Content::OutOfTime => "out of time".into(),
            Content::Withheld => "withheld".into(),
            Content::Conflicted => "conflicted".into(),
            Content::Submodule => "submodule".into(),
            Content::Unreadable(why) => format!("unreadable: {why}"),
        };
        f.debug_struct("Reading")
            .field("path", &String::from_utf8_lossy(&self.entry.path))
            .field("kind", &kind)
            .finish()
    }
}

/// What a reading holds of a file.
pub(crate) enum Content {
    /// Both sides, and the hunks between them.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the sides are refined and visited from ADR 0122's step 3c"
        )
    )]
    Text {
        shape: Arc<Shape>,
        old: Arc<[u8]>,
        new: Arc<[u8]>,
    },
    /// A side is binary: only the sizes are shown.
    Binary { old: u64, new: u64 },
    /// A side is past a limit, the review's room, or the diff past its deadline: only the
    /// sizes are shown.
    TooLarge { old: u64, new: u64 },
    /// The request's time for diffing ran out before this file; the next refresh reads it.
    OutOfTime,
    /// Its name, or its name before a rename, may hold secrets, or it is a work tree file with
    /// more than one name, another of which may: it was not read.
    Withheld,
    /// It is conflicted: neither side was read.
    Conflicted,
    /// A submodule: the entry's ids are what it shows.
    Submodule,
    /// A side could not be read; why, with no file's text. The next refresh reads it again.
    Unreadable(String),
}

/// Hands a report to the event loop; `false` once it is gone.
type SendReport = Box<dyn FnMut(Report) -> bool + Send>;

/// A review's thread. Dropping it stops the thread between files and ends it.
pub(crate) struct Reviewer {
    requests: Sender<Request>,
    /// Requests sent and not yet taken, so that the thread stops one for a newer.
    waiting: Arc<AtomicUsize>,
    live: Arc<AtomicBool>,
}

impl Reviewer {
    /// Starts the thread for the project at `root`, making its index copies under `state`,
    /// oso's own directory in the per-user state directory, and handing each report to `send`.
    pub(crate) fn start(
        root: TrustedRoot,
        state: Option<PathBuf>,
        send: impl FnMut(Report) -> bool + Send + 'static,
    ) -> std::io::Result<Reviewer> {
        let (requests, queue) = mpsc::channel::<Request>();
        let waiting = Arc::new(AtomicUsize::new(0));
        let live = Arc::new(AtomicBool::new(true));
        let send: SendReport = Box::new(send);
        let (thread_waiting, thread_live) = (Arc::clone(&waiting), Arc::clone(&live));
        thread::Builder::new()
            .name("oso-review".into())
            .stack_size(STACK)
            .spawn(move || {
                serve(
                    &queue,
                    send,
                    &root,
                    state.as_deref(),
                    &thread_waiting,
                    &thread_live,
                );
            })?;
        Ok(Reviewer {
            requests,
            waiting,
            live,
        })
    }

    /// Asks the thread for `request`; `false` if it has ended.
    pub(crate) fn ask(&self, request: Request) -> bool {
        self.waiting.fetch_add(1, Ordering::SeqCst);
        let sent = self.requests.send(request).is_ok();
        if !sent {
            self.waiting.fetch_sub(1, Ordering::SeqCst);
        }
        sent
    }
}

impl Drop for Reviewer {
    fn drop(&mut self) {
        self.live.store(false, Ordering::SeqCst);
    }
}

/// Answers each request in turn until the reviewer is dropped or the event loop has gone.
/// Refreshes waiting one after another are answered as one, their shown files joined.
fn serve(
    queue: &Receiver<Request>,
    mut send: SendReport,
    root: &TrustedRoot,
    state: Option<&Path>,
    waiting: &AtomicUsize,
    live: &AtomicBool,
) {
    let mut reader = work::Reader::new(root.path(), state);
    let mut next = None;
    loop {
        let request = match next.take() {
            Some(request) => request,
            None => match queue.recv() {
                Ok(request) => {
                    waiting.fetch_sub(1, Ordering::SeqCst);
                    request
                }
                Err(_) => return,
            },
        };
        let request = match request {
            Request::Refresh { mut first } => {
                while let Ok(newer) = queue.try_recv() {
                    waiting.fetch_sub(1, Ordering::SeqCst);
                    match newer {
                        Request::Refresh { first: more } => first.extend(more),
                        other => {
                            next = Some(other);
                            break;
                        }
                    }
                }
                Request::Refresh { first }
            }
            other => other,
        };
        if !live.load(Ordering::SeqCst) {
            return;
        }
        let asked = asked(&request);
        if let Err(why) = root.check() {
            send(Report::Failed { asked, why });
            continue;
        }
        let stop = || {
            !live.load(Ordering::SeqCst)
                || next.is_some()
                || waiting.load(Ordering::SeqCst) > 0
                || root.revoked()
        };
        let mut gone = false;
        let answered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            reader.answer(request, &stop, &mut |report| {
                gone = gone || !send(report);
                !gone
            });
        }));
        if answered.is_err() {
            log::warn!("a review's request panicked");
            let why = "the review's thread failed".into();
            gone = gone || !send(Report::Failed { asked, why });
            reader = reader.fresh();
        }
        if gone {
            return;
        }
    }
}

fn asked(request: &Request) -> Asked {
    match request {
        Request::Open(_) => Asked::Open,
        Request::Refresh { .. } => Asked::Refresh,
        Request::Baseline => Asked::Baseline,
        Request::Carry(_) => Asked::Carry,
    }
}
