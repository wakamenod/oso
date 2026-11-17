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
//! - Carrying comments is no newer request: carries wait in a queue of their own, answered
//!   between the files of a running request and when nothing runs, so that a file's comments
//!   never stop a refresh (ADR 0122, step 3b). Before each, the root is checked as before a
//!   request, and a newer request stops the run of them, the rest waiting.
//! - A look (step 4) is no newer request either: looks waiting are one, answered when nothing
//!   else waits. It opens each reviewed work tree file as a reading does, reading nothing, to
//!   compare its stamp, and each second look lists the scope again; it says only whether the
//!   review is stale, and a newer request stops it.
//! - Limits, each saying what it hides: 10,000 files and 32 MiB of list; a side of 8 MiB and
//!   200,000 lines, diffed within 1 s, and 5 s of diffing per request; 64 MiB of sides and
//!   hunks held per review; a git run of 10 s.
//! - The thread's stack is [`STACK`]: Myers' diff recurses, and needs up to half a MiB at the
//!   line limit.
//! - Nothing here logs a file's text; `Debug` of a reading gives its path and kind.
//! - Its runner, [`TrustedRoot`] and its secret-name check are the `git` module's, shared with
//!   the worktree jobs (ADR 0121) and the git status (`vcs`, ADR 0127).
//! - Two things need no thread: `message` composes the message of the person's comments to an
//!   agent session (step 12), and [`hunk_header`] writes a hunk's header for it and the view.

mod batch;
pub(crate) mod count;
mod diff;
mod index;
mod list;
#[cfg(test)]
mod look_tests;
pub(crate) mod message;
#[cfg(test)]
mod queue_tests;
mod read;
pub(crate) mod refine;
mod scope;
mod side;
pub(crate) mod stage;
#[cfg(test)]
pub(crate) mod tests;
pub(crate) mod view;
mod work;

use std::cell::RefCell;
use std::collections::VecDeque;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use oso_core::review::{Carried, Carry, Hunk, Shape};

use crate::git::{Id, TrustedRoot};

pub(crate) use list::Entry;
pub(crate) use list::Status;
pub(crate) use scope::Scope;

/// The largest side read: a blob or a work tree file past it shows its size only.
pub(crate) const MAX_BLOB: u64 = 8 * 1024 * 1024;
/// The review thread's stack.
const STACK: usize = 16 * 1024 * 1024;

/// What the review's thread is asked to do.
pub(crate) enum Request {
    /// Review `scope`: list its files and read each.
    Open(Scope),
    /// List again and read the files whose signature changed, those in `first` before the
    /// rest.
    Refresh { first: Vec<Box<[u8]>> },
    /// Take a baseline of the work tree for the session scope.
    Baseline,
    /// Carry comments to a new reading (`oso_core::review::Review::carry`). It stops nothing,
    /// and is answered between the files of a running request.
    Carry(Carry),
    /// Put hunks in the index or take them out (step 5). It stops nothing, and is answered once
    /// the running request ends.
    Stage(stage::Stage),
    /// Look whether what the review shows is stale (step 4). It stops nothing; looks waiting
    /// are one, answered when nothing else waits.
    Look,
}

/// Which request a failure answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Asked {
    Open,
    Refresh,
    Baseline,
    Carry,
    Stage,
    Look,
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
        carried: Box<Carried>,
        cut_short: bool,
    },
    /// A look found the review stale: a reviewed file, the listing, the index or `HEAD` moved.
    Stale,
    /// What the stage with the host's ticket `ticket` did, or why it could not run.
    Staged { ticket: u64, staged: stage::Staged },
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
            Self::Stale => f.write_str("Stale"),
            Self::Carried { cut_short, .. } => f
                .debug_struct("Carried")
                .field("cut_short", cut_short)
                .finish_non_exhaustive(),
            // A patch holds the file's text: only whether it went in.
            Self::Staged { ticket, staged } => f
                .debug_struct("Staged")
                .field("ticket", ticket)
                .field("applied", &staged.is_ok())
                .finish_non_exhaustive(),
            Self::Failed { asked, why } => f
                .debug_struct("Failed")
                .field("asked", asked)
                .field("why", why)
                .finish(),
        }
    }
}

/// A hunk's header, as git writes one with no context: `@@ -40,3 +40,7 @@`.
pub(crate) fn hunk_header(hunk: &Hunk) -> String {
    let side = |start: u32, count: usize| {
        if count == 0 {
            format!("{},0", start.saturating_sub(1))
        } else if count == 1 {
            format!("{start}")
        } else {
            format!("{start},{count}")
        }
    };
    format!(
        "@@ -{} +{} @@",
        side(hunk.old_start, hunk.removed.len()),
        side(hunk.new_start, hunk.added.len())
    )
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

/// What waits for the thread, taken from the queue: carries and stages in order, and the
/// first other request, which stops the one running.
#[derive(Default)]
struct Waiting {
    carries: VecDeque<Carry>,
    stages: VecDeque<stage::Stage>,
    /// Whether a look waits.
    look: bool,
    next: Option<Request>,
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

    /// Asks the thread for `request`; `false` if it has ended. A carry is not counted as
    /// waiting, so it stops nothing.
    pub(crate) fn ask(&self, request: Request) -> bool {
        send_request(&self.requests, &self.waiting, request)
    }
}

/// Sends `request` on `requests`, counted in `waiting` unless it is a carry or a stage; `false`
/// if the thread has ended.
fn send_request(requests: &Sender<Request>, waiting: &AtomicUsize, request: Request) -> bool {
    let counted = !matches!(
        request,
        Request::Carry(_) | Request::Stage(_) | Request::Look
    );
    if counted {
        waiting.fetch_add(1, Ordering::SeqCst);
    }
    let sent = requests.send(request).is_ok();
    if !sent && counted {
        waiting.fetch_sub(1, Ordering::SeqCst);
    }
    sent
}

impl Drop for Reviewer {
    fn drop(&mut self) {
        self.live.store(false, Ordering::SeqCst);
    }
}

/// Answers each request in turn until the reviewer is dropped or the event loop has gone.
/// Refreshes waiting one after another are answered as one, their shown files joined. Carries
/// are answered between files while something runs, and when nothing waits to.
fn serve(
    queue: &Receiver<Request>,
    mut send: SendReport,
    root: &TrustedRoot,
    state: Option<&Path>,
    waiting: &AtomicUsize,
    live: &AtomicBool,
) {
    let mut reader = work::Reader::new(root.path(), state);
    let held = RefCell::new(Waiting::default());
    loop {
        let Some(request) = next_request(queue, &held, waiting) else {
            return;
        };
        if !live.load(Ordering::SeqCst) {
            return;
        }
        let asked = asked(&request);
        // A stage is answered by its ticket however it ends, so the host matches each.
        let ticket = match &request {
            Request::Stage(stage) => Some(stage.ticket),
            _ => None,
        };
        let failed = |why: String| match ticket {
            Some(ticket) => Report::Staged {
                ticket,
                staged: Err(why),
            },
            None => Report::Failed { asked, why },
        };
        if let Err(why) = root.check() {
            if !send(failed(why)) {
                return;
            }
            continue;
        }
        let stop = || {
            !live.load(Ordering::SeqCst)
                || held.borrow().next.is_some()
                || waiting.load(Ordering::SeqCst) > 0
                || root.revoked()
        };
        // Each carry is checked as a request is, and a newer request stops the run of them: the
        // rest wait, to be answered between its files, or failed if the root was revoked.
        let mut carries = || {
            pull(queue, &held, waiting);
            if held.borrow().carries.is_empty() || stop() || root.replaced() {
                return None;
            }
            held.borrow_mut().carries.pop_front()
        };
        let mut gone = false;
        let answered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            reader.answer(request, &stop, &mut carries, &mut |report| {
                gone = gone || !send(report);
                !gone
            });
        }));
        if answered.is_err() {
            log::warn!("a review's request panicked");
            let why = "the review's thread failed".to_owned();
            gone = gone || !send(failed(why));
            reader = reader.fresh();
        }
        if gone {
            return;
        }
    }
}

/// Takes what the queue holds now: carries aside, after those waiting, until another request
/// comes, which waits as the next.
fn pull(queue: &Receiver<Request>, held: &RefCell<Waiting>, waiting: &AtomicUsize) {
    let mut held = held.borrow_mut();
    while held.next.is_none() {
        match queue.try_recv() {
            Ok(Request::Carry(carry)) => held.carries.push_back(carry),
            Ok(Request::Stage(stage)) => held.stages.push_back(stage),
            Ok(Request::Look) => held.look = true,
            Ok(other) => {
                waiting.fetch_sub(1, Ordering::SeqCst);
                held.next = Some(other);
            }
            Err(_) => break,
        }
    }
}

/// The next request to answer: a request taken from the queue before, which stopped the one
/// that ran, then a carry waiting, else the next from the queue, waiting for it. Refreshes that
/// wait one after another become one, their shown files joined; carries among them wait aside,
/// to be answered between its files. `None` once the reviewer is gone.
fn next_request(
    queue: &Receiver<Request>,
    held: &RefCell<Waiting>,
    waiting: &AtomicUsize,
) -> Option<Request> {
    // A stage first, one short git run, so that refreshes coming one after another while
    // the agent edits never keep it waiting. Each borrow ends before the next.
    let stage = held.borrow_mut().stages.pop_front();
    if let Some(stage) = stage {
        return Some(Request::Stage(stage));
    }
    let next = held.borrow_mut().next.take();
    if next.is_none() {
        let carry = held.borrow_mut().carries.pop_front();
        if let Some(carry) = carry {
            return Some(Request::Carry(carry));
        }
        // A look last, when nothing else waits: a refresh coming reads what it would find.
        if std::mem::take(&mut held.borrow_mut().look) {
            return Some(Request::Look);
        }
    }
    let request = match next {
        Some(request) => request,
        None => match queue.recv().ok()? {
            request @ (Request::Carry(_) | Request::Stage(_) | Request::Look) => {
                return Some(request);
            }
            request => {
                waiting.fetch_sub(1, Ordering::SeqCst);
                request
            }
        },
    };
    let Request::Refresh { mut first } = request else {
        return Some(request);
    };
    while let Ok(newer) = queue.try_recv() {
        match newer {
            Request::Carry(carry) => held.borrow_mut().carries.push_back(carry),
            Request::Stage(stage) => held.borrow_mut().stages.push_back(stage),
            Request::Look => held.borrow_mut().look = true,
            Request::Refresh { first: more } => {
                waiting.fetch_sub(1, Ordering::SeqCst);
                first.extend(more);
            }
            other => {
                waiting.fetch_sub(1, Ordering::SeqCst);
                held.borrow_mut().next = Some(other);
                break;
            }
        }
    }
    Some(Request::Refresh { first })
}

fn asked(request: &Request) -> Asked {
    match request {
        Request::Open(_) => Asked::Open,
        Request::Refresh { .. } => Asked::Refresh,
        Request::Baseline => Asked::Baseline,
        Request::Carry(_) => Asked::Carry,
        Request::Stage(_) => Asked::Stage,
        Request::Look => Asked::Look,
    }
}
