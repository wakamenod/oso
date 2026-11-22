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
//! - A jump to the source (step 3, part three) waits with the carries, the latest alone, and is
//!   answered as one is: where a line is in the file's text now, or a side built as a text.
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
//!   hunks held per review; a git run of 10 s, but a pull request's fetch, which has none.
//! - The thread's stack is [`STACK`]: Myers' diff recurses, and needs up to half a MiB at the
//!   line limit.
//! - Nothing here logs a file's text; `Debug` of a reading gives its path and kind.
//! - Its runner, [`TrustedRoot`] and its secret-name check are the `git` module's, shared with
//!   the worktree jobs (ADR 0121) and the git status (`vcs`, ADR 0127).
//! - A pull request's sides come from `gh` and, for commits this clone lacks, a fetch that
//!   writes objects alone, with the review's empty hooks, on open and on each refresh the
//!   person asks for (`pr`, step 12b); carries are answered while they wait.
//! - Three things need no thread: `message` composes the message of the person's comments to an
//!   agent session (step 12), [`hunk_header`] writes a hunk's header for it and the view, and
//!   `saved` is a saved review's file format (step 6), which the app's store reads and writes.

pub(crate) mod ask;
mod batch;
pub(crate) mod commits;
pub(crate) mod count;
mod debug;
mod diff;
mod index;
pub(crate) mod jump;
mod list;
#[cfg(test)]
mod look_tests;
pub(crate) mod message;
pub(crate) mod pr;
#[cfg(test)]
mod queue_tests;
mod read;
pub(crate) mod refine;
pub(crate) mod saved;
mod scope;
mod side;
pub(crate) mod stage;
#[cfg(test)]
pub(crate) mod tests;
pub(crate) mod view;
mod work;

use std::cell::RefCell;
use std::collections::VecDeque;
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
    /// rest. `person` if the person asked for it (`g`, or opening it again), which alone asks
    /// `gh` again for a pull request (step 12b, Q10); a refresh of step 4's marks reads the
    /// sides `gh` last gave.
    Refresh { first: Vec<Box<[u8]>>, person: bool },
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
    /// Find where a jump to the source lands (step 3, part three). It stops nothing, and waits
    /// with the carries, answered between the files of a running request.
    Jump(jump::Jump),
    /// Find what changed in a pull request since its last review (ADR 0167, step 2). It stops
    /// nothing; the latest alone waits, answered when nothing else does, as a look is, and a
    /// newer request stops it.
    Since(commits::SinceAsk),
}

/// What waits aside, answered between the files of a running request: a carry or a jump, in
/// the order asked.
pub(crate) enum Aside {
    Carry(Carry),
    Jump(jump::Jump),
}

impl Aside {
    fn into_request(self) -> Request {
        match self {
            Aside::Carry(carry) => Request::Carry(carry),
            Aside::Jump(jump) => Request::Jump(jump),
        }
    }
}

#[cfg(test)]
impl Request {
    /// A refresh the person asked for, `first` read first.
    pub(crate) fn person_refresh(first: Vec<Box<[u8]>>) -> Request {
        Request::Refresh {
            first,
            person: true,
        }
    }
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
    Jump,
    Since,
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
    /// What `gh` said of the pull request reviewed, before its listing (step 12b).
    PrFound(pr::PrFound),
    /// A fetch of pull request `number`'s commits from `remote` starts (step 12b).
    Fetching { number: u64, remote: String },
    /// Where the jump with the host's ticket landed, or why it could not.
    Jumped(jump::Jumped),
    /// A pull request's commits, after what `gh` said of it (ADR 0167).
    Commits(Box<commits::Listed>),
    /// The message of the commit a review of one commit shows, once its id is known or moved.
    Message(Arc<commits::Message>),
    /// What changed in a pull request since its last review (step 2).
    Since(Box<commits::SinceFound>),
}

/// `text` on one line, at most `max` characters, with no control character.
pub(crate) fn one_line(text: &str, max: usize) -> String {
    let mut out: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(max)
        .collect();
    if text.chars().nth(max).is_some() {
        out.push('\u{2026}');
    }
    out
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

/// What a reading holds of a file.
pub(crate) enum Content {
    /// Both sides, the hunks between them, and the words refinement marks in the hunks.
    Text {
        shape: Arc<Shape>,
        old: Arc<[u8]>,
        new: Arc<[u8]>,
        refined: refine::Refined,
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

/// What waits for the thread, taken from the queue: carries and jumps aside, and stages, each in
/// order, a look and an ask of what changed since, and the first other request, which stops the
/// one running.
#[derive(Default)]
struct Waiting {
    asides: VecDeque<Aside>,
    stages: VecDeque<stage::Stage>,
    /// Whether a look waits.
    look: bool,
    /// What changed since a pull request's last review, the latest asked, waiting (step 2).
    since: Option<commits::SinceAsk>,
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

/// Whether `request` is counted as waiting, so that it stops the one running: not a carry, a
/// stage, a look or a jump.
fn counted(request: &Request) -> bool {
    !matches!(
        request,
        Request::Carry(_)
            | Request::Stage(_)
            | Request::Look
            | Request::Jump(_)
            | Request::Since(_)
    )
}

/// Sends `request` on `requests`, counted in `waiting` unless it is a carry, a stage, a look, a
/// jump or an ask of what changed since; `false` if the thread has ended.
fn send_request(requests: &Sender<Request>, waiting: &AtomicUsize, request: Request) -> bool {
    let counted = counted(&request);
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
        // A stage and a jump are answered by their ticket however they end, so the host
        // matches each.
        let ticket = match &request {
            Request::Stage(stage) => Some((stage.ticket, true)),
            Request::Jump(jump) => Some((jump.ticket, false)),
            _ => None,
        };
        let failed = |why: String| match ticket {
            Some((ticket, true)) => Report::Staged {
                ticket,
                staged: Err(why),
            },
            Some((ticket, false)) => Report::Jumped(jump::Jumped {
                ticket,
                landed: Err(why),
            }),
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
        let mut asides = || {
            pull(queue, &held, waiting);
            if held.borrow().asides.is_empty() || stop() || root.replaced() {
                return None;
            }
            held.borrow_mut().asides.pop_front()
        };
        let mut gone = false;
        let answered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            reader.answer(request, &stop, &mut asides, &mut |report| {
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

/// Holds `jump` aside, dropping any jump waiting there: only the latest one counts, as the
/// host takes the answer to its latest alone.
fn hold_jump(asides: &mut VecDeque<Aside>, jump: jump::Jump) {
    asides.retain(|aside| !matches!(aside, Aside::Jump(_)));
    asides.push_back(Aside::Jump(jump));
}

/// Takes what the queue holds now: carries and jumps aside, after those waiting, until another request
/// comes, which waits as the next.
fn pull(queue: &Receiver<Request>, held: &RefCell<Waiting>, waiting: &AtomicUsize) {
    let mut held = held.borrow_mut();
    while held.next.is_none() {
        match queue.try_recv() {
            Ok(Request::Carry(carry)) => held.asides.push_back(Aside::Carry(carry)),
            Ok(Request::Jump(jump)) => hold_jump(&mut held.asides, jump),
            Ok(Request::Since(ask)) => held.since = Some(ask),
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
        let aside = held.borrow_mut().asides.pop_front();
        if let Some(aside) = aside {
            return Some(aside.into_request());
        }
        // What changed since, then a look, when nothing else waits: a reading goes first.
        let since = held.borrow_mut().since.take();
        if let Some(ask) = since {
            return Some(Request::Since(ask));
        }
        // A look last, when nothing else waits: a refresh coming reads what it would find.
        if std::mem::take(&mut held.borrow_mut().look) {
            return Some(Request::Look);
        }
    }
    let request = match next {
        Some(request) => request,
        None => {
            let request = queue.recv().ok()?;
            if !counted(&request) {
                return Some(request);
            }
            waiting.fetch_sub(1, Ordering::SeqCst);
            request
        }
    };
    let Request::Refresh {
        mut first,
        mut person,
    } = request
    else {
        return Some(request);
    };
    while let Ok(newer) = queue.try_recv() {
        match newer {
            Request::Carry(carry) => held.borrow_mut().asides.push_back(Aside::Carry(carry)),
            Request::Jump(jump) => hold_jump(&mut held.borrow_mut().asides, jump),
            Request::Since(ask) => held.borrow_mut().since = Some(ask),
            Request::Stage(stage) => held.borrow_mut().stages.push_back(stage),
            Request::Look => held.borrow_mut().look = true,
            Request::Refresh {
                first: more,
                person: asked,
            } => {
                waiting.fetch_sub(1, Ordering::SeqCst);
                first.extend(more);
                person |= asked;
            }
            other => {
                waiting.fetch_sub(1, Ordering::SeqCst);
                held.borrow_mut().next = Some(other);
                break;
            }
        }
    }
    Some(Request::Refresh { first, person })
}

fn asked(request: &Request) -> Asked {
    match request {
        Request::Open(_) => Asked::Open,
        Request::Refresh { .. } => Asked::Refresh,
        Request::Baseline => Asked::Baseline,
        Request::Carry(_) => Asked::Carry,
        Request::Stage(_) => Asked::Stage,
        Request::Look => Asked::Look,
        Request::Jump(_) => Asked::Jump,
        Request::Since(_) => Asked::Since,
    }
}
