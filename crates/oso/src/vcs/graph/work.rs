//! A graph buffer's thread, `oso-vcs-graph` (ADR 0127, step 4): it reads the commits of the
//! refs asked for a page at a time, lays out their lanes, and lays out the buffer's text.
//!
//! Must not depend on: `app`, `dispatch`, winit or the UI thread; the caller hands requests in,
//! takes reports out, and supplies the wake-up.
//!
//! - It starts only from a [`TrustedRoot`], looks at the root before each request, and finds
//!   git as the status's thread does: the repository's top must be the root, git runs with the
//!   empty hooks directory and a read's variables, and each read is bounded and killed past ten
//!   seconds.
//! - The lanes go on from page to page: a page lays out after the rows already read, and only
//!   its own rows are written; what is kept is the text and the lanes, not the commits. A read
//!   starts again from the first page.
//! - A page that adds no commit, past the graph's most, or one whose single record is past what
//!   a page reads, ends the paging; a page asked when none can be read is answered at once.
//! - Only the latest read is answered: a read asked while one is under way replaces it.
//! - A page that panics leaves the thread as it was at its start, and says so.

use std::ffi::OsStr;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, SystemTime};

use super::lanes::Lanes;
use super::write::{Laid, Text};
use super::{MAX_COMMITS, MAX_PAGE, PAGE, args, date, parse};
use crate::dired::Offsets;
use crate::git::{Git, TrustedRoot};
use crate::vcs::read::{find, hooks_dir};

/// What the thread is asked.
pub(crate) enum GraphRequest {
    /// Read the first page of `refs` again, under the heading `title`.
    Read {
        generation: u64,
        refs: Vec<String>,
        title: String,
    },
    /// Read the next page, if git has more.
    More { generation: u64 },
}

/// What the thread found.
pub(crate) enum GraphReport {
    /// The text of every row read so far, for the request `generation`.
    Laid { generation: u64, laid: Laid },
    /// No page could be read: the graph holds all it will.
    Unchanged { generation: u64 },
    /// Why nothing could be read.
    Failed { generation: u64, why: String },
}

impl std::fmt::Debug for GraphReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Laid { generation, .. } => write!(f, "Laid({generation})"),
            Self::Unchanged { generation } => write!(f, "Unchanged({generation})"),
            Self::Failed { generation, .. } => write!(f, "Failed({generation})"),
        }
    }
}

/// A graph buffer's thread. Dropping it ends the thread once its read is done.
pub(crate) struct GraphWorker {
    requests: Sender<GraphRequest>,
}

impl GraphWorker {
    /// Starts the thread for the project at `root`, with the empty hooks directory under
    /// `state`, handing each report to `send`, which says `false` once the event loop has gone.
    pub(crate) fn start(
        root: TrustedRoot,
        state: Option<PathBuf>,
        send: impl FnMut(GraphReport) -> bool + Send + 'static,
    ) -> std::io::Result<GraphWorker> {
        let (requests, queue) = mpsc::channel();
        thread::Builder::new()
            .name("oso-vcs-graph".into())
            .spawn(move || serve(&queue, send, &root, state))?;
        Ok(GraphWorker { requests })
    }

    /// Asks the thread for `request`; `false` if it has ended.
    pub(crate) fn ask(&self, request: GraphRequest) -> bool {
        self.requests.send(request).is_ok()
    }
}

/// What the thread keeps between requests: the rows read, the lanes after them, where the next
/// page starts and the refs it reads.
#[derive(Default)]
struct State {
    git: Option<Git>,
    refs: Vec<String>,
    title: String,
    text: Text,
    lanes: Lanes,
    more: bool,
    /// The local zone's offsets, asked once an hour of time.
    offsets: Offsets,
}

fn serve(
    queue: &Receiver<GraphRequest>,
    mut send: impl FnMut(GraphReport) -> bool,
    root: &TrustedRoot,
    state_dir: Option<PathBuf>,
) {
    let mut state = State::default();
    while let Ok(first) = queue.recv() {
        // The last read asked meanwhile is answered, and a page asked after it waits for the
        // host to ask again; with no read, one page.
        let mut request = first;
        while let Ok(newer) = queue.try_recv() {
            if matches!(newer, GraphRequest::Read { .. })
                || matches!(request, GraphRequest::More { .. })
            {
                request = newer;
            }
        }
        let (generation, read) = match request {
            GraphRequest::Read {
                generation,
                refs,
                title,
            } => {
                state.refs = refs;
                state.title = title;
                state.text = Text::default();
                state.lanes = Lanes::default();
                state.more = true;
                (generation, true)
            }
            GraphRequest::More { generation } => (generation, false),
        };
        let paged = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            page(&mut state, root, state_dir.as_deref(), read)
        }));
        let paged = paged.unwrap_or_else(|_| {
            log::warn!("a graph's page panicked");
            state = State::default();
            Err("the graph's thread failed: g reads it again".into())
        });
        let report = match paged {
            Ok(true) => GraphReport::Laid {
                generation,
                laid: state.text.laid(&state.title, state.more),
            },
            Ok(false) => GraphReport::Unchanged { generation },
            Err(why) => GraphReport::Failed { generation, why },
        };
        if !send(report) {
            return;
        }
    }
}

/// Reads the next page into `state`, the first if `first`; nothing past [`MAX_COMMITS`].
/// Whether its text changed.
fn page(
    state: &mut State,
    root: &TrustedRoot,
    state_dir: Option<&std::path::Path>,
    first: bool,
) -> Result<bool, String> {
    root.check()?;
    if state.git.is_none() {
        let hooks = hooks_dir(state_dir)?;
        state.git = Some(find(root.path(), &hooks)?);
    }
    let Some(git) = state.git.as_ref() else {
        return Err("no git".into());
    };
    let read = state.text.rows();
    if !first && (!state.more || read >= MAX_COMMITS) {
        return Ok(false);
    }
    let words = args(&state.refs, read);
    let words: Vec<&OsStr> = words.iter().map(OsStr::new).collect();
    let out = git
        .output(&words, &[], MAX_PAGE)
        .map_err(|e| e.to_string())?;
    if !out.cut && !out.status.success() {
        let said = out.stderr.lines().find(|line| !line.trim().is_empty());
        return Err(said.unwrap_or("git log failed").to_owned());
    }
    let whole = if out.cut {
        (out.stdout.iter())
            .rposition(|&byte| byte == 0)
            .map_or(0, |at| at + 1)
    } else {
        out.stdout.len()
    };
    let room = MAX_COMMITS - read;
    let (commits, more) = parse(&out.stdout[..whole], PAGE.min(room));
    // A page cut before its first whole record would read the same again: it ends the paging.
    state.more = more || (out.cut && !commits.is_empty());
    for commit in &commits {
        let (cells, _) = state.lanes.row(&commit.id, &commit.parents);
        // A crafted time past what the system's clock holds takes no offset.
        let at = SystemTime::UNIX_EPOCH
            .checked_add(Duration::from_secs(commit.time.max(0).unsigned_abs()));
        let offset = at.map_or(0, |at| state.offsets.at(at));
        state.text.push(&cells, commit, &date(commit.time, offset));
    }
    Ok(first || !commits.is_empty())
}

#[cfg(test)]
mod tests;
