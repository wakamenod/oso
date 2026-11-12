//! Searching a project's files for `project-search` (ADR 0114): a thread of its own, `oso-grep`,
//! walks the files `project-find-file` lists and hands each to searching threads, which read it
//! and match its lines.
//!
//! Must not depend on: `app`, winit or the UI thread; this reads the file system, and the caller
//! supplies the wake-up.
//!
//! - The walk is [`walk::walk_files`]'s, with its rules and limits. A link is listed there but
//!   never read here. A file is opened from its directory, as the walk opened it, following no
//!   link and waiting for nothing, and only a regular file is read.
//! - A file is read in blocks of [`BLOCK`] bytes cut at a line's end. A longer line grows its
//!   block up to [`MAX_LINE`], past which the rest of the file is skipped. A NUL byte stops the
//!   file, as `rg` stops at a binary one, keeping the rows found before it.
//! - Each request has a generation, which the walk reads at each entry and a searcher before
//!   each file and each block: a superseded search stops there and sends nothing more.
//! - A file's rows go together and in line order; files come in the order they are finished. A
//!   batch goes at most every [`BATCH_EVERY`]. At [`MAX_ROWS`] the search stops and says it was
//!   cut.
//! - A file that may hold secrets, by its path's name, is searched, but its rows carry no text.
//! - The lines a search that ended without a limit found are kept here, up to [`MAX_KEPT`]
//!   bytes, so that a request that narrows it looks only through them.
//! - Nothing here logs what is searched for or found; `Debug` gives a generation and a count.

use std::cell::Cell;
use std::ffi::OsString;
use std::fmt;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use oso_core::buffer::Buffer;
use oso_core::editor::{SearchRequest, SearchRow, WalkEnd};
use oso_core::project::{BLOCK, LineQuery, MAX_LINE, MAX_ROWS, row_text};
use oso_platform::file_meta::Directory;

use crate::walk::{self, Ended, Found as Walked, Limits, Visit};

/// How often a search sends the rows it found.
pub const BATCH_EVERY: Duration = Duration::from_millis(30);

/// The most searching threads.
const MAX_SEARCHERS: usize = 8;

/// The most files waiting for a searcher, each holding its directory open.
const QUEUED: usize = 32;

/// The most bytes of lines kept for a search that narrows the last one.
pub const MAX_KEPT: usize = 16 * 1024 * 1024;

/// A batch of rows a search found, and whether it ended. `Debug` gives its generation and size
/// only.
pub struct Found {
    pub generation: u64,
    pub rows: Vec<SearchRow>,
    pub end: Option<WalkEnd>,
}

impl fmt::Debug for Found {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Found")
            .field("generation", &self.generation)
            .field("rows", &self.rows.len())
            .field("end", &self.end)
            .finish()
    }
}

/// Hands a batch to the event loop; `false` once it is gone.
type SendRows = Arc<dyn Fn(Found) -> bool + Send + Sync>;

/// A line a search found, kept whole for a search that narrows it.
struct Kept {
    path: Box<str>,
    line: usize,
    bytes: Box<[u8]>,
    secret: bool,
}

/// What one search has found so far, shared by its searchers and `oso-grep`.
#[derive(Default)]
struct Collected {
    rows: Vec<SearchRow>,
    /// Rows found in all, sent or not.
    total: usize,
    kept: Vec<Kept>,
    kept_bytes: usize,
    /// More lines were found than are kept.
    kept_over: bool,
    /// Files handed out and not finished yet.
    pending: usize,
}

/// One search, shared by its searchers and `oso-grep`.
struct Search {
    request: SearchRequest,
    collected: Mutex<Collected>,
    finished: Condvar,
    /// It reached [`MAX_ROWS`].
    full: AtomicBool,
}

impl Search {
    fn live(&self) -> bool {
        self.request.live.load(Ordering::Relaxed) == self.request.generation
            && !self.full.load(Ordering::Relaxed)
    }

    fn collected(&self) -> std::sync::MutexGuard<'_, Collected> {
        self.collected
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Takes in the rows of one file, as many as fit under [`MAX_ROWS`], and the lines they
    /// came from.
    fn file_done(&self, rows: Vec<SearchRow>, kept: Vec<Kept>) {
        let mut collected = self.collected();
        let room = MAX_ROWS - collected.total;
        if rows.len() > room {
            self.full.store(true, Ordering::Relaxed);
        }
        collected.total += rows.len().min(room);
        collected.rows.extend(rows.into_iter().take(room));
        if !collected.kept_over {
            let bytes: usize = kept.iter().map(|kept| kept.bytes.len()).sum();
            if collected.kept_bytes + bytes > MAX_KEPT {
                collected.kept_over = true;
                collected.kept = Vec::new();
            } else {
                collected.kept_bytes += bytes;
                collected.kept.extend(kept);
            }
        }
        collected.pending -= 1;
        drop(collected);
        self.finished.notify_all();
    }
}

/// A file for a searcher to read.
struct Task {
    search: Arc<Search>,
    directory: Arc<Directory>,
    name: OsString,
    path: Box<str>,
    secret: bool,
}

/// The searching thread and its searchers, started with the first request.
pub struct Grep {
    requests: Option<Sender<SearchRequest>>,
    send: SendRows,
}

impl Grep {
    /// A searcher that hands each batch to `send`.
    pub fn new(send: impl Fn(Found) -> bool + Send + Sync + 'static) -> Self {
        Self {
            requests: None,
            send: Arc::new(send),
        }
    }

    /// Searches as `request` asks, on `oso-grep`, after any search before it, which a newer
    /// one has stopped. If the thread cannot start, the search ends at once with nothing found,
    /// so the prompt stops waiting.
    pub fn search(&mut self, request: SearchRequest) {
        if self.requests.is_none() {
            let (requests, queue) = mpsc::channel::<SearchRequest>();
            let send = Arc::clone(&self.send);
            let spawned = thread::Builder::new()
                .name("oso-grep".into())
                .spawn(move || serve(&queue, &*send));
            if spawned.is_ok() {
                self.requests = Some(requests);
            } else {
                log::warn!("the searching thread could not start");
            }
        }
        let request = match &self.requests {
            Some(requests) => match requests.send(request) {
                Ok(()) => return,
                Err(failed) => failed.0,
            },
            None => request,
        };
        self.requests = None;
        (self.send)(Found {
            generation: request.generation,
            rows: Vec::new(),
            end: Some(WalkEnd { cut: false }),
        });
    }
}

/// Starts the searchers, which take files from the queue returned; none if none could start.
fn searchers() -> Option<SyncSender<Task>> {
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

/// Reads each file handed out until `oso-grep` has gone.
fn search_files(queue: &Mutex<Receiver<Task>>) {
    loop {
        let task = queue.lock().unwrap_or_else(PoisonError::into_inner).recv();
        let Ok(task) = task else {
            return;
        };
        let search = Arc::clone(&task.search);
        let found = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(task)));
        let (rows, kept) = found.unwrap_or_else(|_| {
            // The panic hook writes no message, so nothing of the file is printed.
            log::warn!("a search of a file panicked");
            (Vec::new(), Vec::new())
        });
        search.file_done(rows, kept);
    }
}

/// Searches the file of `task`, unless its search is over.
fn run(task: Task) -> (Vec<SearchRow>, Vec<Kept>) {
    if !task.search.live() {
        return (Vec::new(), Vec::new());
    }
    let Ok(Some((file, _))) = task.directory.open_file(&task.name) else {
        return (Vec::new(), Vec::new());
    };
    let mut rows = Vec::new();
    let mut kept = Vec::new();
    let query = &task.search.request.query;
    search_file(
        file,
        query,
        &|| task.search.live(),
        &mut |line, bytes, first| {
            let (text, column) = row_text(bytes, first);
            rows.push(SearchRow {
                path: task.path.clone(),
                line,
                column,
                text: if task.secret { "".into() } else { text },
                secret: task.secret,
            });
            kept.push(Kept {
                path: task.path.clone(),
                line,
                bytes: bytes.into(),
                secret: task.secret,
            });
            rows.len() < MAX_ROWS
        },
    );
    (rows, kept)
}

/// What a matching line is handed to: its number from 1, its bytes and where its first match
/// is. It says whether to go on.
pub(crate) type Hit<'a> = dyn FnMut(usize, &[u8], std::ops::Range<usize>) -> bool + 'a;

/// Reads `file` as the module says, handing each line that matches `query` to `hit`, with its
/// number from 1, its bytes and where its first match is, until `hit` says no more or `live`
/// says the search is over.
pub(crate) fn search_file(
    mut file: impl Read,
    query: &LineQuery,
    live: &dyn Fn() -> bool,
    hit: &mut Hit<'_>,
) {
    let mut block: Vec<u8> = Vec::with_capacity(BLOCK);
    // The number of the block's first line, from 1.
    let mut first_line = 1;
    let mut ended = false;
    // How much of the block was looked at for a NUL and a line break.
    let mut scanned = 0;
    loop {
        if !live() {
            return;
        }
        // Read until the block holds a whole block's bytes past what was carried, or the file
        // ends. It never holds more than the longest line read, so a longer one never fits.
        let wanted = (block.len() + BLOCK).min(MAX_LINE);
        while block.len() < wanted && !ended {
            let start = block.len();
            block.resize(wanted, 0);
            match file.read(&mut block[start..]) {
                Ok(0) | Err(_) => {
                    block.truncate(start);
                    ended = true;
                }
                Ok(read) => block.truncate(start + read),
            }
        }
        // A NUL stops the file: only the lines before its own are searched. What was looked at
        // already holds neither a NUL nor a line break.
        let nul = block[scanned..]
            .iter()
            .position(|&byte| byte == 0)
            .map(|at| scanned + at);
        let (searched, last) = match nul {
            Some(at) => {
                let newline = block[..at].iter().rposition(|&byte| byte == b'\n');
                (newline.map_or(0, |newline| newline + 1), true)
            }
            None if ended => (block.len(), true),
            None => match block[scanned..].iter().rposition(|&byte| byte == b'\n') {
                Some(newline) => (scanned + newline + 1, false),
                // One line longer than a block: it grows, up to its bound.
                None if block.len() < MAX_LINE => {
                    scanned = block.len();
                    continue;
                }
                None => return,
            },
        };
        let mut more = true;
        query.find_in_block(&block[..searched], |found| {
            if more {
                let bytes = &block[found.range.clone()];
                more = hit(first_line + found.line, bytes, found.first);
            }
        });
        if last || !more {
            return;
        }
        first_line += block[..searched].iter().filter(|&&b| b == b'\n').count();
        block.drain(..searched);
        scanned = block.len();
    }
}

/// Searches as each request that comes asks, in turn, until the searcher is dropped or the
/// event loop has gone.
fn serve(queue: &Receiver<SearchRequest>, send: &(dyn Fn(Found) -> bool + Send + Sync)) {
    let mut tasks = None;
    // The lines the last search that ended without a limit found, and its generation.
    let mut kept: Option<(u64, Vec<Kept>)> = None;
    while let Ok(request) = queue.recv() {
        let generation = request.generation;
        if request.live.load(Ordering::Relaxed) != generation {
            continue;
        }
        if tasks.is_none() {
            tasks = searchers();
        }
        let narrowed = match (request.narrow, &kept) {
            (Some(of), Some((generation, _))) => of == *generation,
            _ => false,
        };
        let search = Arc::new(Search {
            request,
            collected: Mutex::new(Collected::default()),
            finished: Condvar::new(),
            full: AtomicBool::new(false),
        });
        let lines = kept.as_ref().filter(|_| narrowed).map(|(_, lines)| lines);
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match lines {
            Some(lines) => filter(&search, lines, send),
            None => walk_and_search(&search, tasks.as_ref(), send),
        }));
        let Ok(outcome) = outcome else {
            log::warn!("a search panicked");
            let end = Some(WalkEnd { cut: false });
            if !send(Found {
                generation,
                rows: Vec::new(),
                end,
            }) {
                return;
            }
            continue;
        };
        match outcome {
            Outcome::Gone => return,
            // The lines kept stay for a search that narrows them still.
            Outcome::Stopped => {}
            Outcome::Ended(whole) => kept = whole.map(|lines| (generation, lines)),
        }
    }
}

/// How a search ended.
enum Outcome {
    /// The event loop has gone.
    Gone,
    /// A newer one took its place.
    Stopped,
    /// It ended, with the lines it found if it found them all and they are few enough to keep.
    Ended(Option<Vec<Kept>>),
}

/// Sends the rows waiting, if [`BATCH_EVERY`] has passed since `sent`. `false` once the event
/// loop has gone.
fn flush(search: &Search, sent: &mut Instant, send: &dyn Fn(Found) -> bool) -> bool {
    if sent.elapsed() < BATCH_EVERY {
        return true;
    }
    let rows = std::mem::take(&mut search.collected().rows);
    if rows.is_empty() {
        return true;
    }
    *sent = Instant::now();
    send(Found {
        generation: search.request.generation,
        rows,
        end: None,
    })
}

/// Walks the project and hands each file to a searcher, or reads it here if none could start.
fn walk_and_search(
    search: &Arc<Search>,
    tasks: Option<&SyncSender<Task>>,
    send: &dyn Fn(Found) -> bool,
) -> Outcome {
    struct Handing<'a> {
        search: &'a Arc<Search>,
        tasks: Option<&'a SyncSender<Task>>,
        sent: Instant,
        send: &'a dyn Fn(Found) -> bool,
        gone: &'a Cell<bool>,
    }
    impl Visit for Handing<'_> {
        fn file(&mut self, found: Walked<'_>) {
            if found.link {
                return;
            }
            let root = &self.search.request.root;
            let secret = Buffer::name_may_hold_secrets(&root.join(&*found.path));
            let task = Task {
                search: Arc::clone(self.search),
                directory: Arc::clone(found.directory),
                name: found.name.to_os_string(),
                path: found.path,
                secret,
            };
            self.search.collected().pending += 1;
            let task = match self.tasks {
                Some(tasks) => match tasks.send(task) {
                    Ok(()) => return,
                    Err(failed) => failed.0,
                },
                None => task,
            };
            let search = Arc::clone(&task.search);
            let (rows, kept) = run(task);
            search.file_done(rows, kept);
        }

        fn entry(&mut self) {
            if !flush(self.search, &mut self.sent, self.send) {
                self.gone.set(true);
            }
        }
    }
    let gone = Cell::new(false);
    let mut handing = Handing {
        search,
        tasks,
        sent: Instant::now(),
        send,
        gone: &gone,
    };
    let root = &search.request.root;
    let stop = || !search.live() || gone.get();
    let walked = walk::walk_files(root, Limits::PROJECT, &stop, &mut handing);
    if gone.get() {
        return Outcome::Gone;
    }
    finish(search, handing.sent, walked, send)
}

/// Waits for the files a search handed out, sending what they find, and sends the rest once
/// they are done, with whether the search was cut.
fn finish(
    search: &Search,
    mut sent: Instant,
    walked: Result<(), Ended>,
    send: &dyn Fn(Found) -> bool,
) -> Outcome {
    // The files handed out finish, or the search is superseded.
    loop {
        let collected = search.collected();
        if collected.pending == 0 {
            break;
        }
        let (collected, _) = search
            .finished
            .wait_timeout(collected, BATCH_EVERY)
            .unwrap_or_else(PoisonError::into_inner);
        drop(collected);
        if search.request.live.load(Ordering::Relaxed) != search.request.generation {
            return Outcome::Stopped;
        }
        if !flush(search, &mut sent, send) {
            return Outcome::Gone;
        }
    }
    let full = search.full.load(Ordering::Relaxed);
    let cut = match walked {
        Ok(()) => full,
        Err(Ended::Cut) => true,
        Err(Ended::Stopped) if full => true,
        Err(Ended::Stopped) => return Outcome::Stopped,
    };
    if search.request.live.load(Ordering::Relaxed) != search.request.generation {
        return Outcome::Stopped;
    }
    let mut collected = search.collected();
    let rows = std::mem::take(&mut collected.rows);
    let whole = (!cut && !collected.kept_over).then(|| std::mem::take(&mut collected.kept));
    drop(collected);
    let end = Some(WalkEnd { cut });
    if !send(Found {
        generation: search.request.generation,
        rows,
        end,
    }) {
        return Outcome::Gone;
    }
    Outcome::Ended(whole)
}

/// Looks through `lines`, the ones the search before found, for those this one matches.
fn filter(search: &Search, lines: &[Kept], send: &dyn Fn(Found) -> bool) -> Outcome {
    let query = &search.request.query;
    let mut rows = Vec::new();
    let mut whole = Vec::new();
    let mut sent = Instant::now();
    for (index, line) in lines.iter().enumerate() {
        if index % 1024 == 0 && !search.live() {
            return Outcome::Stopped;
        }
        let Some(first) = query.matches_line(&line.bytes) else {
            continue;
        };
        let (text, column) = row_text(&line.bytes, first);
        rows.push(SearchRow {
            path: line.path.clone(),
            line: line.line,
            column,
            text: if line.secret { "".into() } else { text },
            secret: line.secret,
        });
        whole.push(Kept {
            path: line.path.clone(),
            line: line.line,
            bytes: line.bytes.clone(),
            secret: line.secret,
        });
        if sent.elapsed() >= BATCH_EVERY {
            let batch = std::mem::take(&mut rows);
            if !send(Found {
                generation: search.request.generation,
                rows: batch,
                end: None,
            }) {
                return Outcome::Gone;
            }
            sent = Instant::now();
        }
    }
    let end = Some(WalkEnd { cut: false });
    if !send(Found {
        generation: search.request.generation,
        rows,
        end,
    }) {
        return Outcome::Gone;
    }
    Outcome::Ended(Some(whole))
}

#[cfg(test)]
mod tests;
