//! The searching threads of a project's search (ADR 0114): the files handed to them, what one
//! search has found so far, and the lines kept for a search that narrows it.
//!
//! Must not depend on: `app`, winit or the UI thread; this reads the files it is handed.
//!
//! What a search may hold is counted as it is found, not once a file is done: its rows against
//! [`MAX_ROWS`], and the bytes of the lines it keeps against [`MAX_KEPT`], so neither one file nor
//! all searchers at once can make it grow past its bound.

use std::ffi::OsString;
use std::io::ErrorKind;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;

use oso_core::editor::{SearchRequest, SearchRow};
use oso_core::project::{LineQuery, MAX_ROWS, row_text};
use oso_platform::file_meta::Directory;

use super::read::search_file;

/// The most searching threads.
const MAX_SEARCHERS: usize = 8;

/// The most files waiting for a searcher, each holding its directory open.
pub(super) const QUEUED: usize = 16;

/// The most bytes of lines, and of their paths, kept for a search that narrows the last one.
pub const MAX_KEPT: usize = 16 * 1024 * 1024;

/// A line a search found, kept whole for a search that narrows it.
pub(super) struct Kept {
    path: Arc<str>,
    line: usize,
    pub(super) bytes: Arc<[u8]>,
    secret: bool,
}

/// The row of the line `line` of the file at `path`, `bytes`, whose first match is at `first`:
/// with no text if the file may hold secrets.
fn row(path: &str, line: usize, bytes: &[u8], first: Range<usize>, secret: bool) -> SearchRow {
    let (text, column) = row_text(bytes, first);
    SearchRow {
        path: path.into(),
        line,
        column,
        text: if secret { "".into() } else { text },
        secret,
    }
}

impl Kept {
    /// Its row, whose first match is at `first`.
    pub(super) fn row(&self, first: Range<usize>) -> SearchRow {
        row(&self.path, self.line, &self.bytes, first, self.secret)
    }

    /// The same line, kept again, sharing its bytes.
    pub(super) fn again(&self) -> Self {
        Self {
            path: Arc::clone(&self.path),
            line: self.line,
            bytes: Arc::clone(&self.bytes),
            secret: self.secret,
        }
    }
}

/// What one search has found and not sent yet, and the lines it keeps.
#[derive(Default)]
pub(super) struct Collected {
    pub(super) rows: Vec<SearchRow>,
    pub(super) kept: Vec<Kept>,
}

/// One search, shared by its searchers and `oso-grep`.
pub(super) struct Search {
    pub(super) request: SearchRequest,
    collected: Mutex<Collected>,
    finished: Condvar,
    /// Files handed out and not finished yet.
    pending: AtomicUsize,
    /// Rows taken in all, sent or not, each as it is found.
    total: AtomicUsize,
    /// Bytes of the lines kept.
    kept_bytes: AtomicUsize,
    /// More lines were found than are kept.
    pub(super) kept_over: AtomicBool,
    /// It reached [`MAX_ROWS`].
    pub(super) full: AtomicBool,
    /// A file could not be read for a reason other than its being gone, refused or no file, as
    /// when the process has no descriptor left: the search says it left some out.
    pub(super) incomplete: AtomicBool,
}

impl Search {
    pub(super) fn new(request: SearchRequest) -> Self {
        Self {
            request,
            collected: Mutex::new(Collected::default()),
            finished: Condvar::new(),
            pending: AtomicUsize::new(0),
            total: AtomicUsize::new(0),
            kept_bytes: AtomicUsize::new(0),
            kept_over: AtomicBool::new(false),
            full: AtomicBool::new(false),
            incomplete: AtomicBool::new(false),
        }
    }

    /// Whether this is still the newest search.
    pub(super) fn newest(&self) -> bool {
        self.request.live.load(Ordering::Relaxed) == self.request.generation
    }

    /// Whether it goes on: the newest, and not full.
    pub(super) fn live(&self) -> bool {
        self.newest() && !self.full.load(Ordering::Relaxed)
    }

    pub(super) fn collected(&self) -> MutexGuard<'_, Collected> {
        self.collected
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// A file was handed out.
    pub(super) fn handed_out(&self) {
        self.pending.fetch_add(1, Ordering::Relaxed);
    }

    /// Waits for the files handed out, up to `wait`. Whether none is left.
    pub(super) fn wait_for_files(&self, wait: std::time::Duration) -> bool {
        let collected = self.collected();
        if self.pending.load(Ordering::Relaxed) == 0 {
            return true;
        }
        let _ = self
            .finished
            .wait_timeout(collected, wait)
            .unwrap_or_else(PoisonError::into_inner);
        self.pending.load(Ordering::Relaxed) == 0
    }

    /// Whether a line found that costs `cost`, its bytes and its path's, may be kept: within
    /// [`MAX_KEPT`] for the whole search.
    pub(super) fn may_keep(&self, cost: usize) -> bool {
        if self.kept_over.load(Ordering::Relaxed) {
            return false;
        }
        if self.kept_bytes.fetch_add(cost, Ordering::Relaxed) + cost > MAX_KEPT {
            self.kept_over.store(true, Ordering::Relaxed);
            return false;
        }
        true
    }

    /// Takes one more row, if the search has room for it under [`MAX_ROWS`]: otherwise it is
    /// full, and stops.
    pub(super) fn take_row(&self) -> bool {
        if self.total.fetch_add(1, Ordering::Relaxed) < MAX_ROWS {
            return true;
        }
        self.full.store(true, Ordering::Relaxed);
        false
    }

    /// Takes in the rows of one file, each taken as it was found, and the lines they came from.
    pub(super) fn file_done(&self, rows: Vec<SearchRow>, kept: Vec<Kept>) {
        let mut collected = self.collected();
        collected.rows.extend(rows);
        if self.kept_over.load(Ordering::Relaxed) {
            collected.kept = Vec::new();
        } else {
            collected.kept.extend(kept);
        }
        self.pending.fetch_sub(1, Ordering::Relaxed);
        drop(collected);
        self.finished.notify_all();
    }
}

/// A file for a searcher to read.
pub(super) struct Task {
    pub(super) search: Arc<Search>,
    pub(super) directory: Arc<Directory>,
    pub(super) name: OsString,
    pub(super) path: Box<str>,
    pub(super) secret: bool,
}

/// Starts the searchers, which take files from the queue returned; none if none could start.
pub(super) fn searchers() -> Option<SyncSender<Task>> {
    let count = thread::available_parallelism()
        .map_or(1, |cores| cores.get().saturating_sub(1))
        .clamp(1, MAX_SEARCHERS);
    let (tasks, queue) = mpsc::sync_channel::<Task>(QUEUED);
    let queue = Arc::new(Mutex::new(queue));
    let started = (0..count)
        .filter(|index| {
            let queue = Arc::clone(&queue);
            thread::Builder::new()
                .name(format!("oso-grep-{index}"))
                .spawn(move || search_files(&queue))
                .is_ok()
        })
        .count();
    (started > 0).then_some(tasks)
}

/// What a searcher keeps from file to file: its own copy of the search's query, whose
/// expressions then keep their caches to this thread, and its block.
#[derive(Default)]
pub(super) struct Searcher {
    query: Option<(u64, LineQuery)>,
    block: Vec<u8>,
}

/// Reads each file handed out until `oso-grep` has gone.
fn search_files(queue: &Mutex<Receiver<Task>>) {
    let mut searcher = Searcher::default();
    loop {
        let task = queue.lock().unwrap_or_else(PoisonError::into_inner).recv();
        let Ok(task) = task else {
            return;
        };
        let search = Arc::clone(&task.search);
        let found = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| searcher.run(task)));
        let (rows, kept) = found.unwrap_or_else(|_| {
            // The panic hook writes no message, so nothing of the file is printed.
            log::warn!("a search of a file panicked");
            searcher = Searcher::default();
            (Vec::new(), Vec::new())
        });
        search.file_done(rows, kept);
    }
}

impl Searcher {
    /// Searches the file of `task`, unless its search is over.
    pub(super) fn run(&mut self, task: Task) -> (Vec<SearchRow>, Vec<Kept>) {
        let search = &task.search;
        if !search.live() {
            return (Vec::new(), Vec::new());
        }
        let file = match task.directory.open_file(&task.name) {
            Ok(Some((file, metadata))) if metadata.len() > 0 => file,
            Ok(_) => return (Vec::new(), Vec::new()),
            Err(error) => {
                let expected = matches!(
                    error.kind(),
                    ErrorKind::NotFound | ErrorKind::PermissionDenied
                );
                if !expected {
                    search.incomplete.store(true, Ordering::Relaxed);
                }
                return (Vec::new(), Vec::new());
            }
        };
        let generation = search.request.generation;
        if self.query.as_ref().is_none_or(|(of, _)| *of != generation) {
            self.query = Some((generation, LineQuery::clone(&search.request.query)));
        }
        let Some((_, query)) = &self.query else {
            return (Vec::new(), Vec::new());
        };
        let path: Arc<str> = Arc::from(&*task.path);
        let mut rows = Vec::new();
        let mut kept = Vec::new();
        let mut hit = |line, bytes: &[u8], first| {
            // A file stops at what is left of the search's rows, which every searcher shares.
            if !search.take_row() {
                return false;
            }
            rows.push(row(&path, line, bytes, first, task.secret));
            // A line past what may be kept is not copied.
            if search.may_keep(bytes.len() + path.len()) {
                kept.push(Kept {
                    path: Arc::clone(&path),
                    line,
                    bytes: bytes.into(),
                    secret: task.secret,
                });
            } else {
                kept = Vec::new();
            }
            true
        };
        if !search_file(file, query, &|| search.live(), &mut hit, &mut self.block) {
            search.incomplete.store(true, Ordering::Relaxed);
        }
        (rows, kept)
    }
}

/// Looks through `lines`, the ones the search before found, for those `search` matches, handing
/// each batch of rows to `send` as [`BATCH_EVERY`](super::BATCH_EVERY) passes. `None` if a newer
/// search took its place, or the event loop has gone, which `gone` says.
pub(super) fn filter(
    search: &Search,
    lines: &[Kept],
    send: &mut dyn FnMut(Vec<SearchRow>) -> bool,
) -> Result<Vec<Kept>, bool> {
    let query = LineQuery::clone(&search.request.query);
    let mut rows = Vec::new();
    let mut whole = Vec::new();
    let mut sent = std::time::Instant::now();
    for (index, line) in lines.iter().enumerate() {
        if index % 1024 == 0 && !search.live() {
            return Err(false);
        }
        let Some(first) = query.matches_line(&line.bytes) else {
            continue;
        };
        rows.push(line.row(first));
        whole.push(line.again());
        if sent.elapsed() >= super::BATCH_EVERY {
            if !send(std::mem::take(&mut rows)) {
                return Err(true);
            }
            sent = std::time::Instant::now();
        }
    }
    if !send(rows) {
        return Err(true);
    }
    Ok(whole)
}
