//! Searching a project's files for `project-search` (ADR 0114): a thread of its own, `oso-grep`,
//! walks the files `project-find-file` lists and hands each to searching threads, which read it
//! and match its lines. The selected row's preview is read in `grep/preview.rs`.
//!
//! Must not depend on: `app`, winit or the UI thread; this reads the file system, and the caller
//! supplies the wake-up.
//!
//! - The walk is [`walk::walk_files`]'s, with its rules and limits, and no root that holds the
//!   home directory is searched, by the real place of either. A link is never read. A file is
//!   opened from its directory, as the walk opened it, following no link and waiting for
//!   nothing, and only a regular file is read.
//! - A file is read in blocks of [`BLOCK`](oso_core::project::BLOCK) bytes cut at a line's end
//!   (`read.rs`). A NUL byte stops the file, as `rg` stops at a binary one.
//! - Each request has a generation, which the walk reads at each entry and a searcher before
//!   each file and each block: a superseded search stops there and sends nothing more.
//! - A file's rows go together and in line order; files come in the order they are finished. A
//!   batch goes at most every [`BATCH_EVERY`], of at most [`MAX_BATCH`] rows. At
//!   [`MAX_ROWS`](oso_core::project::MAX_ROWS) the search stops and says it was cut, and so does
//!   one that could not read some file for a reason other than its being gone or refused.
//! - A file that may hold secrets, by the name of its path from the root as given or as it
//!   leads, is searched, but its rows carry no text.
//! - The lines a search that ended without a limit found are kept here, up to
//!   [`MAX_KEPT`](files::MAX_KEPT) bytes, so that a request that narrows it looks only through
//!   them. They go once no search came for [`KEPT_FOR`].
//! - Nothing here logs what is searched for or found; `Debug` gives a generation and a count.

use std::cell::Cell;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::thread;
use std::time::{Duration, Instant};

use oso_core::buffer::Buffer;
use oso_core::editor::{ProjectSearchRequest, ProjectSearchRow, WalkEnd};

use oso_platform::file_meta::Directory;

use crate::walk::{self, Ended, File, Limits, Visit};
use files::{Kept, Search, Searcher, Task, filter, searchers};

mod files;
mod preview;
mod read;

pub use preview::{Previewed, Previewer};

/// How often a search sends the rows it found.
pub const BATCH_EVERY: Duration = Duration::from_millis(30);

/// The most rows one batch takes to the editor, whose UI thread takes each in well under a
/// millisecond.
pub const MAX_BATCH: usize = 1000;

/// How long the lines a search found are kept with no search coming.
pub const KEPT_FOR: Duration = Duration::from_secs(30);

/// A batch of rows a search found, and whether it ended. `Debug` gives its generation and size
/// only.
pub struct Found {
    pub generation: u64,
    pub rows: Vec<ProjectSearchRow>,
    pub end: Option<WalkEnd>,
}

impl Found {
    /// The end of the search `generation`, with nothing more found.
    fn ended(generation: u64) -> Self {
        Self {
            generation,
            rows: Vec::new(),
            end: Some(WalkEnd { cut: false }),
        }
    }
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

/// Where a root leads, as the loader finds it: each link's target on the way, then its real
/// place; `None` if nobody can tell. The app hands over the loader's own, which this module, on
/// the loader's layer, does not name.
pub type Leads = fn(&Path) -> Option<Vec<PathBuf>>;

/// Whether the file at `path` from `root` may hold secrets, as the loader would judge it: by its
/// name from the root as given or from any place `leads` says the root leads through. A root
/// nobody could resolve, `None`, is taken to, as the loader takes a path it cannot resolve.
pub(crate) fn may_hold_secrets(root: &Path, leads: Option<&[PathBuf]>, path: &Path) -> bool {
    let named = |root: &Path| Buffer::name_may_hold_secrets(&root.join(path));
    named(root) || leads.is_none_or(|leads| leads.iter().any(|lead| named(lead)))
}

/// Whether `opened`, a root opened by its path, is the directory `leads` says the path leads to,
/// by device and inode, so that what was judged is what is read. One that cannot be told is not.
#[cfg(unix)]
pub(crate) fn opened_as_judged(opened: &Directory, leads: Option<&[PathBuf]>) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Some(real) = leads.and_then(<[PathBuf]>::last) else {
        return false;
    };
    match (opened.metadata(), std::fs::metadata(real)) {
        (Ok(opened), Ok(real)) => (opened.dev(), opened.ino()) == (real.dev(), real.ino()),
        _ => false,
    }
}

/// On Windows directories are reached by their paths, so there is nothing to compare.
#[cfg(not(unix))]
pub(crate) fn opened_as_judged(_: &Directory, _: Option<&[PathBuf]>) -> bool {
    true
}

/// The searching thread and its searchers, started with the first request.
pub struct Grep {
    requests: Option<Sender<ProjectSearchRequest>>,
    send: SendRows,
    leads: Leads,
    /// The home directory as the person's environment spells it, which the thread finds the
    /// real place of once.
    home: Option<PathBuf>,
}

impl Grep {
    /// A searcher that hands each batch to `send`, finds where a root leads with `leads`, and
    /// never searches a root that holds `home`, by the real place of either, as the walker never
    /// lists one.
    pub fn new(
        home: Option<PathBuf>,
        leads: Leads,
        send: impl Fn(Found) -> bool + Send + Sync + 'static,
    ) -> Self {
        Self {
            requests: None,
            send: Arc::new(send),
            leads,
            home,
        }
    }

    /// Searches as `request` asks, on `oso-grep`, after any search before it, which a newer
    /// one has stopped. If the thread cannot start, the search ends at once with nothing found,
    /// so the prompt stops waiting.
    pub fn search(&mut self, request: ProjectSearchRequest) {
        if self.requests.is_none() {
            let (requests, queue) = mpsc::channel::<ProjectSearchRequest>();
            let send = Arc::clone(&self.send);
            let (home, leads) = (self.home.clone(), self.leads);
            let spawned = thread::Builder::new()
                .name("oso-grep".into())
                .spawn(move || serve(&queue, &*send, home.as_deref(), leads));
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
        (self.send)(Found::ended(request.generation));
    }
}

/// Searches as each request that comes asks, in turn, until the searcher is dropped or the
/// event loop has gone.
fn serve(
    queue: &Receiver<ProjectSearchRequest>,
    send: &(dyn Fn(Found) -> bool + Send + Sync),
    home: Option<&Path>,
    leads: Leads,
) {
    let home = walk::real_home(home);
    let mut tasks = None;
    // The lines the last search that ended without a limit found, and its generation.
    let mut kept: Option<(u64, Vec<Kept>)> = None;
    loop {
        let request = match queue.recv_timeout(KEPT_FOR) {
            Ok(request) => request,
            Err(RecvTimeoutError::Timeout) => {
                kept = None;
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => return,
        };
        let generation = request.generation;
        if request.live.load(Ordering::Relaxed) != generation {
            continue;
        }
        // Nothing is searched, and the prompt stops waiting.
        if walk::holds_home(&request.root, home.as_deref()) {
            if !send(Found::ended(generation)) {
                return;
            }
            continue;
        }
        if tasks.is_none() {
            tasks = searchers();
        }
        let narrowed = match (request.narrow, &kept) {
            (Some(of), Some((generation, _))) => of == *generation,
            _ => false,
        };
        // A search that walks finds lines of its own, and those kept go first, so that the two
        // are never held together.
        if !narrowed {
            kept = None;
        }
        let search = Arc::new(Search::new(request));
        let lines = kept.as_ref().filter(|_| narrowed).map(|(_, lines)| lines);
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match lines {
            Some(lines) => narrow(&search, lines, send),
            None => walk_and_search(&search, (home.as_deref(), leads), tasks.as_ref(), send),
        }));
        let Ok(outcome) = outcome else {
            log::warn!("a search panicked");
            if !send(Found::ended(generation)) {
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
    send_rows(search.request.generation, rows, None, send)
}

/// Sends `rows` of the search `generation` in batches of at most [`MAX_BATCH`], with `end` on
/// the last. `false` once the event loop has gone.
fn send_rows(
    generation: u64,
    mut rows: Vec<ProjectSearchRow>,
    end: Option<WalkEnd>,
    send: &dyn Fn(Found) -> bool,
) -> bool {
    while rows.len() > MAX_BATCH {
        let rest = rows.split_off(MAX_BATCH);
        let batch = std::mem::replace(&mut rows, rest);
        if !send(Found {
            generation,
            rows: batch,
            end: None,
        }) {
            return false;
        }
    }
    send(Found {
        generation,
        rows,
        end,
    })
}

/// Hands each file a walk finds to a searcher, or reads it here if none could start.
struct Handing<'a> {
    search: &'a Arc<Search>,
    tasks: Option<&'a SyncSender<Task>>,
    /// Where the root leads, whose files' names are judged too.
    leads: Option<Vec<PathBuf>>,
    /// Reads the files here, with no searcher.
    here: Searcher,
    sent: Instant,
    send: &'a dyn Fn(Found) -> bool,
    gone: &'a Cell<bool>,
}

impl Visit for Handing<'_> {
    fn file(&mut self, file: File<'_>) {
        if file.link {
            return;
        }
        let root = &self.search.request.root;
        let secret = may_hold_secrets(root, self.leads.as_deref(), Path::new(&*file.path));
        let task = Task {
            search: Arc::clone(self.search),
            directory: Arc::clone(file.directory),
            name: file.name.to_os_string(),
            path: file.path,
            secret,
        };
        self.search.handed_out();
        let task = match self.tasks {
            Some(tasks) => match tasks.send(task) {
                Ok(()) => return,
                Err(failed) => failed.0,
            },
            None => task,
        };
        let (rows, kept) = self.here.run(task);
        self.search.file_done(rows, kept);
    }

    fn entry(&mut self) {
        if !flush(self.search, &mut self.sent, self.send) {
            self.gone.set(true);
        }
    }

    fn lists_links(&self) -> bool {
        false
    }

    fn root_opened(&mut self, root: &Directory) -> bool {
        opened_as_judged(root, self.leads.as_deref())
    }
}

/// Walks the project and hands each file to a searcher, then sends what they found.
fn walk_and_search(
    search: &Arc<Search>,
    (home, leads): (Option<&Path>, Leads),
    tasks: Option<&SyncSender<Task>>,
    send: &dyn Fn(Found) -> bool,
) -> Outcome {
    let root = &search.request.root;
    let leads = leads(root);
    let gone = Cell::new(false);
    let mut handing = Handing {
        search,
        tasks,
        leads,
        here: Searcher::default(),
        sent: Instant::now(),
        send,
        gone: &gone,
    };
    let stop = || !search.live() || gone.get();
    let walked = walk::walk_files(root, home, Limits::PROJECT, &stop, &mut handing);
    if gone.get() {
        return Outcome::Gone;
    }
    let mut sent = handing.sent;
    // The files handed out finish, or the search is superseded.
    while !search.wait_for_files(BATCH_EVERY) {
        if !search.newest() {
            return Outcome::Stopped;
        }
        if !flush(search, &mut sent, send) {
            return Outcome::Gone;
        }
    }
    let full = search.full.load(Ordering::Relaxed) || search.incomplete.load(Ordering::Relaxed);
    let cut = match walked {
        Ok(()) => full,
        Err(Ended::Cut) => true,
        Err(Ended::Stopped) if full && search.newest() => true,
        Err(Ended::Stopped) => return Outcome::Stopped,
    };
    if !search.newest() {
        return Outcome::Stopped;
    }
    let mut collected = search.collected();
    let rows = std::mem::take(&mut collected.rows);
    let over = search.kept_over.load(Ordering::Relaxed);
    let whole = (!cut && !over).then(|| std::mem::take(&mut collected.kept));
    drop(collected);
    let end = Some(WalkEnd { cut });
    if !send_rows(search.request.generation, rows, end, send) {
        return Outcome::Gone;
    }
    Outcome::Ended(whole)
}

/// Looks through `lines`, the ones the search before found, for those this one matches.
fn narrow(search: &Search, lines: &[Kept], send: &dyn Fn(Found) -> bool) -> Outcome {
    let generation = search.request.generation;
    let filtered = filter(search, lines, &mut |rows| {
        send_rows(generation, rows, None, send)
    });
    match filtered {
        Ok(whole) => {
            if !send(Found::ended(generation)) {
                return Outcome::Gone;
            }
            Outcome::Ended(Some(whole))
        }
        Err(true) => Outcome::Gone,
        Err(false) => Outcome::Stopped,
    }
}

#[cfg(test)]
mod tests;
