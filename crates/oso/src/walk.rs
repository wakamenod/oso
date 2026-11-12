//! Walking a project's files for `project-find-file` and `project-search` (ADR 0114), the
//! first on a thread of its own, `oso-walk`, matching ignore files as ripgrep does.
//!
//! Must not depend on: `app`, winit or the UI thread; this touches the file system, and the
//! caller supplies the wake-up.
//!
//! The files a walk lists are the ones `rg --files --hidden` lists, less what belongs to no
//! file of this project:
//!
//! - Hidden files are listed. The `.git`, `.jj` and `.hg` of the project are never entered,
//!   and a directory that holds one is another project, so it is not entered either. A marker
//!   counts as detection counts it: a `.git` directory or file, a `.jj` or `.hg` directory, and
//!   never a link.
//! - Ignore files apply as they do for `rg`, in its order: `.rgignore`, then `.ignore`, then,
//!   inside a git repository, `.gitignore`, the root's `.git/info/exclude` and the person's
//!   global excludes. The deepest directory's file decides within each. Only the project's
//!   directories are read, none above its root, and the walk reads each file itself, as
//!   patterns only: only a regular file that is not a link, opened without waiting, and at most
//!   [`MAX_IGNORE_FILE`] bytes. A worktree's root, whose `.git` is a file, reads no
//!   `info/exclude`. The global excludes are the person's own, and may be a link, but must be a
//!   regular file of at most as many bytes too.
//! - Each directory below the root is opened from its parent by its name, following no link,
//!   and listed and read from what was opened, never by its path again
//!   (`oso_platform::file_meta::Directory`). At most [`tree::MAX_HELD`] stay open; one let go
//!   is opened again the same way, name by name, if it is still the directory it was.
//! - Links are not followed. A link is resolved by its words, hop by hop, and looked at only
//!   while every hop stays under the root: it is listed if it leads to a file. A link that leaves
//!   the root is listed without a look, and the loader judges it when it is opened. A link that
//!   leads to a directory, or nowhere, is not listed.
//! - A path that is not UTF-8 is left out, as an answer naming it would name another file.
//! - At most [`MAX_WALKED`] files are listed, and at most [`MAX_PATH_BYTES`] of their paths.
//!   At most [`MAX_ENTRIES`] entries are read, ignored ones too, and [`MAX_IGNORE_BYTES`] of
//!   ignore files, and no directory deeper than [`MAX_DEPTH`] or [`MAX_PATH`] bytes is entered.
//!   Past any of them the walk stops and says it was cut, and so it does when a directory cannot
//!   be opened or listed for a reason other than its being gone, refused or no directory.
//!
//! What it finds goes to the editor in batches, one at most every [`BATCH_EVERY`]. A newer
//! listing, `C-g` or closing the prompt moves the request's live generation on, and the walk
//! stops at its next entry. One thread walks every request in turn: a newer one stops the one
//! before first, and a slow or hung file system stalls only this thread.

use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::{Duration, Instant};

use oso_core::editor::{WalkEnd, WalkRequest};
use oso_core::project::{MARKERS, MAX_WALKED};
use oso_platform::file_meta::{Directory, EntryKind};

/// How often a walk sends what it found.
pub const BATCH_EVERY: Duration = Duration::from_millis(50);

/// The most bytes of an ignore file that are read; a longer one is not read at all.
pub const MAX_IGNORE_FILE: u64 = 1024 * 1024;

/// The most bytes of paths a walk lists, as 100,000 paths can each be up to a few KiB long.
pub const MAX_PATH_BYTES: usize = 32 * 1024 * 1024;

/// The most entries a walk reads, listed or not, as an ignored directory of millions of names
/// would be read whole otherwise.
pub const MAX_ENTRIES: usize = 1_000_000;

/// The most bytes of ignore files a walk reads, all of them together.
pub const MAX_IGNORE_BYTES: u64 = 16 * 1024 * 1024;

/// The most directories deep a walk goes, as each one it is in holds its path and its ignore
/// files are asked of each entry below it.
pub const MAX_DEPTH: usize = 256;

/// The most bytes of a directory's path a walk enters, the longest path the system opens.
pub const MAX_PATH: usize = 4096;

/// How much a walk lists and reads at most.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Files listed.
    pub files: usize,
    /// Bytes of the paths listed.
    pub bytes: usize,
    /// Entries read, listed or not.
    pub entries: usize,
    /// Bytes of ignore files read.
    pub ignore_bytes: u64,
}

impl Limits {
    /// A project's: [`MAX_WALKED`] files, [`MAX_PATH_BYTES`] of their paths (ADR 0114),
    /// [`MAX_ENTRIES`] entries and [`MAX_IGNORE_BYTES`] of ignore files.
    pub const PROJECT: Self = Self {
        files: MAX_WALKED,
        bytes: MAX_PATH_BYTES,
        entries: MAX_ENTRIES,
        ignore_bytes: MAX_IGNORE_BYTES,
    };
}

/// What a walk has read so far, against its [`Limits`].
struct Spent {
    limits: Limits,
    /// Entries read, and names looked at to resolve links.
    entries: usize,
    /// Bytes of the names read, against the same bound as the paths listed.
    names: usize,
    ignore_bytes: u64,
}

/// Why a walk ends before it has walked everything.
#[derive(Debug)]
pub(crate) enum Ended {
    /// A newer listing, or the prompt closed.
    Stopped,
    /// It reached a limit.
    Cut,
}

/// The ignore files of a directory, in `rg`'s order: the first that says anything decides.
const IGNORE_FILES: [&str; 3] = [".rgignore", ".ignore", ".gitignore"];

/// A batch of what a walk found: paths from the project's root, and whether the walk ended.
/// `Debug` gives its generation and size only, as a list can name files the person keeps
/// private.
pub struct Walked {
    pub generation: u64,
    pub paths: Vec<Box<str>>,
    pub end: Option<WalkEnd>,
}

impl fmt::Debug for Walked {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Walked")
            .field("generation", &self.generation)
            .field("paths", &self.paths.len())
            .field("end", &self.end)
            .finish()
    }
}

/// The walker's thread, started with the first request.
pub struct Walker {
    requests: Option<Sender<WalkRequest>>,
    send: SendBatch,
    /// The home directory as the person's environment spells it, which the thread finds the
    /// real place of once.
    home: Option<PathBuf>,
}

/// Hands a batch to the event loop; `false` once it is gone.
type SendBatch = std::sync::Arc<dyn Fn(Walked) -> bool + Send + Sync>;

impl Walker {
    /// A walker that hands each batch to `send`, and never walks a root that holds `home`, by
    /// the real place of either: the editor refuses it by its words, which a link can hide.
    pub fn new(
        home: Option<PathBuf>,
        send: impl Fn(Walked) -> bool + Send + Sync + 'static,
    ) -> Self {
        Self {
            requests: None,
            send: std::sync::Arc::new(send),
            home,
        }
    }

    /// Walks `request`'s root on the walker's thread, after any walk before it, which a newer
    /// listing has stopped. If the thread cannot start, the walk ends at once with nothing
    /// found, so the prompt stops waiting.
    pub fn walk(&mut self, request: WalkRequest) {
        if self.requests.is_none() {
            let (requests, queue) = mpsc::channel::<WalkRequest>();
            let send = std::sync::Arc::clone(&self.send);
            let home = self.home.clone();
            let spawned = thread::Builder::new()
                .name("oso-walk".into())
                .spawn(move || serve(&queue, &*send, home.as_deref()));
            if spawned.is_ok() {
                self.requests = Some(requests);
            } else {
                log::warn!("the walking thread could not start");
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
        (self.send)(Walked {
            generation: request.generation,
            paths: Vec::new(),
            end: Some(WalkEnd { cut: false }),
        });
    }
}

/// Whether the directory `root` holds `home`, the home directory's real place, by its own real
/// place: the editor refuses such a root by its words, which a link can hide.
pub(crate) fn holds_home(root: &Path, home: Option<&Path>) -> bool {
    let root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    home.is_some_and(|home| home.starts_with(&root))
}

/// The home directory's real place, or, if that cannot be found, `home` as it is spelled, which
/// a walk still refuses.
pub(crate) fn real_home(home: Option<&Path>) -> Option<PathBuf> {
    home.map(|home| fs::canonicalize(home).unwrap_or_else(|_| home.to_path_buf()))
}

/// Whether `opened`, a walk's root, is `home` or a directory that holds it, by device and inode:
/// whatever the root's path says, and however it was reached since its path was looked at. One
/// that cannot be told is taken to.
#[cfg(unix)]
pub(crate) fn opened_holds_home(opened: &Directory, home: Option<&Path>) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Some(home) = home else {
        return false;
    };
    let Ok(root) = opened.metadata() else {
        return true;
    };
    let root = (root.dev(), root.ino());
    home.ancestors()
        .filter_map(|directory| fs::metadata(directory).ok())
        .any(|directory| (directory.dev(), directory.ino()) == root)
}

#[cfg(not(unix))]
pub(crate) fn opened_holds_home(_: &Directory, _: Option<&Path>) -> bool {
    false
}

/// Walks each request that comes, in turn, until the walker is dropped or the event loop has
/// gone.
fn serve(
    queue: &mpsc::Receiver<WalkRequest>,
    send: &(dyn Fn(Walked) -> bool + Send + Sync),
    home: Option<&Path>,
) {
    let home = real_home(home);
    while let Ok(request) = queue.recv() {
        let generation = request.generation;
        let live = || request.live.load(Ordering::Relaxed) == generation;
        if !live() {
            continue;
        }
        if holds_home(&request.root, home.as_deref()) {
            // Nothing is listed, and the prompt stops waiting.
            let ended = send(Walked {
                generation,
                paths: Vec::new(),
                end: Some(WalkEnd { cut: false }),
            });
            if !ended {
                return;
            }
            continue;
        }
        let mut gone = false;
        let walked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            walk(
                &request.root,
                home.as_deref(),
                Limits::PROJECT,
                &|| !live(),
                &mut |paths, end| {
                    gone |= !send(Walked {
                        generation,
                        paths,
                        end,
                    });
                },
            );
        }));
        if walked.is_err() {
            // The panic hook writes no message, so nothing of the paths is printed.
            log::warn!("a walk panicked");
            gone |= !send(Walked {
                generation,
                paths: Vec::new(),
                end: Some(WalkEnd { cut: false }),
            });
        }
        if gone {
            return;
        }
    }
}

/// A file a walk found: the directory it is in, open, its name there, its path from the root,
/// and whether it is a link, which is listed but never read.
pub(crate) struct File<'a> {
    pub(crate) directory: &'a Arc<Directory>,
    pub(crate) name: &'a OsStr,
    pub(crate) path: Box<str>,
    pub(crate) link: bool,
}

/// What a walk hands what it finds to.
pub(crate) trait Visit {
    /// A file found, within the limits.
    fn file(&mut self, file: File<'_>);

    /// An entry was read, listed or not, so that what waits can be sent in time.
    fn entry(&mut self) {}

    /// Whether links are handed over, which are then resolved by their words; a search, which
    /// never reads one, needs none.
    fn lists_links(&self) -> bool {
        true
    }
}

/// Walks the files under `root` as the module says, listing at most `limits` and nothing of a
/// root that holds `home`, and hands them to
/// `batch` as paths from `root`, every [`BATCH_EVERY`] and once more as it ends. A walk that
/// `stop` stops ends with no last batch.
pub fn walk(
    root: &Path,
    home: Option<&Path>,
    limits: Limits,
    stop: &dyn Fn() -> bool,
    batch: &mut dyn FnMut(Vec<Box<str>>, Option<WalkEnd>),
) {
    struct Batches<'a> {
        found: Vec<Box<str>>,
        sent: Instant,
        batch: &'a mut dyn FnMut(Vec<Box<str>>, Option<WalkEnd>),
    }
    impl Visit for Batches<'_> {
        fn file(&mut self, file: File<'_>) {
            self.found.push(file.path);
        }

        fn entry(&mut self) {
            if self.sent.elapsed() >= BATCH_EVERY && !self.found.is_empty() {
                (self.batch)(std::mem::take(&mut self.found), None);
                self.sent = Instant::now();
            }
        }
    }
    let mut batches = Batches {
        found: Vec::new(),
        sent: Instant::now(),
        batch,
    };
    let cut = match walk_files(root, home, limits, stop, &mut batches) {
        Ok(()) => false,
        Err(Ended::Cut) => true,
        Err(Ended::Stopped) => return,
    };
    (batches.batch)(batches.found, Some(WalkEnd { cut }));
}

/// Walks the files under `root` as the module says, handing each one found to `visit`, at most
/// `limits` of them, and nothing if the root opened is `home`, the home directory's real place,
/// or holds it. It ends with [`Ended::Stopped`] once `stop` says so, asked at each entry, and
/// with [`Ended::Cut`] at a limit.
pub(crate) fn walk_files(
    root: &Path,
    home: Option<&Path>,
    limits: Limits,
    stop: &dyn Fn() -> bool,
    visit: &mut dyn Visit,
) -> Result<(), Ended> {
    let mut spent = Spent {
        limits,
        entries: 0,
        names: 0,
        ignore_bytes: 0,
    };
    // One that cannot be listed lists nothing, as `rg` lists nothing of it.
    let Ok(opened) = Directory::open(root) else {
        return Ok(());
    };
    if opened_holds_home(&opened, home) {
        return Ok(());
    }
    let opened = Arc::new(opened);
    let project = Project::at(root, &opened, &mut spent);
    let mut stack = Stack::new();
    match Listed::open(opened, root.to_path_buf(), &project, &mut spent, stop)? {
        Some(listed) => stack.push(listed),
        None => return Ok(()),
    }
    let (mut count, mut bytes) = (0, 0);
    while let Some(directory) = stack.last_mut() {
        if stop() {
            return Err(Ended::Stopped);
        }
        let Some((name, kind)) = directory.entries.pop_front() else {
            stack.pop();
            continue;
        };
        visit.entry();
        let path = directory.path.join(&name);
        // The project's own markers, and any marker's name below, are never listed or entered.
        if MARKERS.iter().any(|marker| name == *marker) {
            continue;
        }
        let is_dir = kind == EntryKind::Directory;
        if project.ignores(stack.listed(), &path, is_dir) {
            continue;
        }
        let link = kind == EntryKind::Link;
        if !(is_dir || kind == EntryKind::File || link && visit.lists_links()) {
            continue;
        }
        // The directory it is in, open again if it was let go.
        let Some(here) = stack.open_top(&mut spent)? else {
            continue;
        };
        if is_dir {
            // Deeper than any path the system takes, a walk stops, as each level holds the path.
            if stack.listed().len() > MAX_DEPTH || path.as_os_str().len() > MAX_PATH {
                return Err(Ended::Cut);
            }
            // One that is gone, refused or no directory now is left out; one that could not be
            // opened for another reason, as when no descriptor is left, ends the walk cut.
            let opened = match here.open_directory(&name) {
                Ok(Some(opened)) => opened,
                Ok(None) => continue,
                Err(error) if tree::expected(&error) => continue,
                Err(_) => return Err(Ended::Cut),
            };
            let opened = Arc::new(opened);
            if let Some(listed) = Listed::below(opened, path, name, &project, &mut spent, stop)? {
                stack.push(listed);
            }
            continue;
        }
        let Some(relative) = listed(root, stack.root(), &path, link, &mut spent)? else {
            continue;
        };
        if count == limits.files || bytes + relative.len() > limits.bytes {
            return Err(Ended::Cut);
        }
        bytes += relative.len();
        count += 1;
        visit.file(File {
            directory: &here,
            name: &name,
            path: relative,
            link,
        });
    }
    Ok(())
}

/// The path from `root` that the entry at `path`, a file or a link if `link`, is listed as: a
/// file, or a link that leads to one or leaves the root, with a UTF-8 path. `opened` is the root,
/// open, which a link is resolved from.
fn listed(
    root: &Path,
    opened: &Directory,
    path: &Path,
    link: bool,
    spent: &mut Spent,
) -> Result<Option<Box<str>>, Ended> {
    let Ok(from_root) = path.strip_prefix(root) else {
        return Ok(None);
    };
    if link {
        let mut looks = spent.limits.entries - spent.entries;
        let leads = resolve(opened, root, from_root, &mut looks);
        spent.entries = spent.limits.entries - looks;
        let file = match leads {
            Leads::Inside(file) => file,
            Leads::Outside => true,
            Leads::Nowhere => false,
            Leads::Spent => return Err(Ended::Cut),
        };
        if !file {
            return Ok(None);
        }
    }
    let Some(relative) = from_root.to_str() else {
        return Ok(None);
    };
    // One spelling on every system: `/` is no name's on Windows either.
    #[cfg(windows)]
    let relative = relative.replace('\\', "/");
    Ok(Some(relative.into()))
}

mod links;
mod tree;

use links::{Leads, resolve};
use tree::{Listed, Project, Stack};

#[cfg(test)]
mod tests;
