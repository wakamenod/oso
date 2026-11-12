//! A review's git and diffing (ADR 0122): one thread per review, `oso-review`, that runs its
//! git commands in order, reads each file's sides, diffs them with `similar` and hands each
//! file's reading to the event loop.
//!
//! Must not depend on: `app`, `dispatch`, winit or the UI thread; the caller hands requests in
//! and takes reports out, and supplies the wake-up.
//!
//! - It starts only from a [`TrustedRoot`], which only a trusted project gives: a restricted
//!   project runs no git (ADR 0009, ADR 0011).
//! - git runs only through `oso-platform::process`, with oso's arguments, and its output is
//!   read as bytes, each read bounded ([`git`], [`list`]).
//! - Old sides, and new sides that are no work tree file, come through one long-lived
//!   `git cat-file --batch` ([`batch`]); work tree files are read from the work tree's top a
//!   directory at a time, following no link ([`side`]).
//! - A file whose name may hold secrets is listed, with its counts, and never read.
//! - A file whose signature did not change since the last reading is not read again, and the
//!   files the caller shows go first.
//! - Limits, each saying what it hides: 10,000 files and 32 MiB of list; a side of 8 MiB and
//!   200,000 lines, diffed within 1 s; 64 MiB of sides held per review; a git run of 10 s.
//! - Nothing here logs a file's text; `Debug` of a reading gives its path and kind.

mod batch;
mod diff;
mod git;
mod index;
mod list;
mod read;
mod scope;
mod side;
#[cfg(test)]
mod tests;

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use oso_core::editor::Editor;
use oso_core::project::ProjectId;
use oso_core::review::{Carried, Carry, Shape};

pub(crate) use git::Id;
pub(crate) use list::Entry;
pub(crate) use scope::Scope;

/// The root of a project that is trusted to run git. Only [`TrustedRoot::of`] makes one.
#[derive(Clone, Debug)]
pub(crate) struct TrustedRoot(PathBuf);

impl TrustedRoot {
    /// The root of project `id`, if the editor trusts it.
    pub(crate) fn of(editor: &Editor, id: ProjectId) -> Option<TrustedRoot> {
        if !editor.project_trusted(id) {
            return None;
        }
        Some(TrustedRoot(editor.project(id)?.root().to_owned()))
    }

    #[cfg(test)]
    pub(crate) fn for_tests(root: PathBuf) -> TrustedRoot {
        TrustedRoot(root)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
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
    Carry(Carry),
}

/// What the review's thread found. Each listing and the readings after it share a generation.
pub(crate) enum Report {
    /// The files the scope differs in, in git's order, and how many more were left out; `cut`
    /// if git wrote more than a review reads.
    Listed {
        generation: u64,
        entries: Vec<Entry>,
        more: usize,
        cut: bool,
    },
    /// One file's reading, new or changed since the last one.
    File {
        generation: u64,
        reading: Arc<Reading>,
    },
    /// A baseline was taken; `left_out` files could not be read.
    Baseline {
        tree: Id,
        left_out: usize,
    },
    /// The session's baseline is gone.
    BaselineGone,
    Carried(Carried),
    /// A request failed; the message says why, with no file's text.
    Failed(String),
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
            Self::Baseline { tree, left_out } => f
                .debug_struct("Baseline")
                .field("tree", tree)
                .field("left_out", left_out)
                .finish(),
            Self::BaselineGone => f.write_str("BaselineGone"),
            Self::Carried(_) => f.write_str("Carried"),
            Self::Failed(why) => f.debug_tuple("Failed").field(why).finish(),
        }
    }
}

/// One reading of a file.
pub(crate) struct Reading {
    pub(crate) entry: Entry,
    pub(crate) content: Content,
}

impl fmt::Debug for Reading {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match &self.content {
            Content::Text { shape, .. } => format!("text, {} hunks", shape.hunks().len()),
            Content::Binary { .. } => "binary".into(),
            Content::TooLarge { .. } => "too large".into(),
            Content::Withheld => "withheld".into(),
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
    Text {
        shape: Arc<Shape>,
        old: Arc<[u8]>,
        new: Arc<[u8]>,
    },
    /// A side is binary: only the sizes are shown.
    Binary { old: u64, new: u64 },
    /// A side is past a limit, or the diff past its deadline: only the sizes are shown.
    TooLarge { old: u64, new: u64 },
    /// Its name may hold secrets: it was not read.
    Withheld,
    /// A submodule: the entry's ids are what it shows.
    Submodule,
    /// A side could not be read; why, with no file's text.
    Unreadable(String),
}

/// Hands a report to the event loop; `false` once it is gone.
type Send_ = Arc<dyn Fn(Report) -> bool + Send + Sync>;

/// A review's thread. Dropping it ends the thread once its current request is done.
pub(crate) struct Reviewer {
    requests: Sender<Request>,
}

impl Reviewer {
    /// Starts the thread for the project at `root`, making its index copies under `state`,
    /// oso's own directory in the per-user state directory, and handing each report to `send`.
    pub(crate) fn start(
        root: TrustedRoot,
        state: Option<PathBuf>,
        send: impl Fn(Report) -> bool + Send + Sync + 'static,
    ) -> std::io::Result<Reviewer> {
        let (requests, queue) = mpsc::channel::<Request>();
        let send: Send_ = Arc::new(send);
        thread::Builder::new()
            .name("oso-review".into())
            .spawn(move || serve(&queue, &*send, &root, state.as_deref()))?;
        Ok(Reviewer { requests })
    }

    /// Asks the thread for `request`; `false` if it has ended.
    pub(crate) fn ask(&self, request: Request) -> bool {
        self.requests.send(request).is_ok()
    }
}

/// Answers each request in turn until the reviewer is dropped or the event loop has gone.
fn serve(
    queue: &Receiver<Request>,
    send: &(dyn Fn(Report) -> bool + Send + Sync),
    root: &TrustedRoot,
    state: Option<&Path>,
) {
    let mut reader = read::Reader::new(root.path(), state);
    while let Ok(request) = queue.recv() {
        let mut gone = false;
        let mut report = |report: Report| {
            gone = gone || !send(report);
            !gone
        };
        let answered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            reader.answer(request, &mut report);
        }));
        if answered.is_err() {
            log::warn!("a review's request panicked");
            report(Report::Failed("the review's thread failed".into()));
            reader = read::Reader::new(root.path(), state);
        }
        if gone {
            return;
        }
    }
}
