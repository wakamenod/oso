//! Walking a project's files for `project-find-file` (ADR 0114), on a thread of its own,
//! `oso-walk`, matching ignore files as ripgrep does.
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
//! - Links are not followed. A link is resolved by its words, hop by hop, and looked at only
//!   while every hop stays under the root: it is listed if it leads to a file. A link that leaves
//!   the root is listed without a look, and the loader judges it when it is opened. A link that
//!   leads to a directory, or nowhere, is not listed.
//! - A path that is not UTF-8 is left out, as an answer naming it would name another file.
//! - At most [`MAX_WALKED`] files are listed, and at most [`MAX_PATH_BYTES`] of their paths.
//!   At most [`MAX_ENTRIES`] entries are read, ignored ones too, and [`MAX_IGNORE_BYTES`] of
//!   ignore files. Past any of them the walk stops and says it was cut.
//!
//! What it finds goes to the editor in batches, one at most every [`BATCH_EVERY`]. A newer
//! listing, `C-g` or closing the prompt moves the request's live generation on, and the walk
//! stops at its next entry. One thread walks every request in turn: a newer one stops the one
//! before first, and a slow or hung file system stalls only this thread.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::fmt;
use std::fs::{self, FileType};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::{Duration, Instant};

use ignore::Match;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use oso_core::editor::{WalkEnd, WalkRequest};
use oso_core::project::{MARKERS, MAX_WALKED};

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
enum Ended {
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

/// Walks each request that comes, in turn, until the walker is dropped or the event loop has
/// gone.
fn serve(
    queue: &mpsc::Receiver<WalkRequest>,
    send: &(dyn Fn(Walked) -> bool + Send + Sync),
    home: Option<&Path>,
) {
    let home = home.and_then(|home| fs::canonicalize(home).ok());
    while let Ok(request) = queue.recv() {
        let generation = request.generation;
        let live = || request.live.load(Ordering::Relaxed) == generation;
        if !live() {
            continue;
        }
        let root = fs::canonicalize(&request.root).unwrap_or_else(|_| request.root.clone());
        if home.as_deref().is_some_and(|home| home.starts_with(&root)) {
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

/// Walks the files under `root` as the module says, listing at most `limits`, and hands them to
/// `batch` as paths from `root`, every [`BATCH_EVERY`] and once more as it ends. A walk that
/// `stop` stops ends with no last batch.
pub fn walk(
    root: &Path,
    limits: Limits,
    stop: &dyn Fn() -> bool,
    batch: &mut dyn FnMut(Vec<Box<str>>, Option<WalkEnd>),
) {
    let mut spent = Spent {
        limits,
        entries: 0,
        names: 0,
        ignore_bytes: 0,
    };
    let project = Project::at(root, &mut spent);
    let mut stack = Vec::new();
    match Directory::open(root, &project, false, &mut spent, stop) {
        Ok(Some(directory)) => stack.push(directory),
        Ok(None) => {}
        Err(Ended::Stopped) => return,
        Err(Ended::Cut) => return batch(Vec::new(), Some(WalkEnd { cut: true })),
    }
    let mut found = Vec::new();
    let (mut count, mut bytes) = (0, 0);
    let mut sent = Instant::now();
    while let Some(directory) = stack.last_mut() {
        if stop() {
            return;
        }
        let Some((name, kind)) = directory.entries.pop_front() else {
            stack.pop();
            continue;
        };
        let path = directory.path.join(&name);
        // The project's own markers, and any marker's name below, are never listed or entered.
        if MARKERS.iter().any(|marker| name == *marker) {
            continue;
        }
        let is_dir = kind.is_dir();
        if project.ignores(&stack, &path, is_dir) {
            continue;
        }
        if is_dir {
            // One that cannot be listed is left out, as `rg` leaves it.
            match Directory::open(&path, &project, true, &mut spent, stop) {
                Ok(Some(directory)) => stack.push(directory),
                Ok(None) => {}
                Err(Ended::Stopped) => return,
                Err(Ended::Cut) => return batch(found, Some(WalkEnd { cut: true })),
            }
        } else if let Some(path) = match listed(root, &path, kind, &mut spent) {
            Ok(listed) => listed,
            Err(_) => return batch(found, Some(WalkEnd { cut: true })),
        } {
            if count == limits.files || bytes + path.len() > limits.bytes {
                batch(found, Some(WalkEnd { cut: true }));
                return;
            }
            bytes += path.len();
            count += 1;
            found.push(path);
        }
        if sent.elapsed() >= BATCH_EVERY && !found.is_empty() {
            batch(std::mem::take(&mut found), None);
            sent = Instant::now();
        }
    }
    batch(found, Some(WalkEnd { cut: false }));
}

/// What applies to a whole project: whether it is a git repository, whose `.gitignore` files
/// count, and its root's `info/exclude` and the person's global excludes, which apply only
/// then.
struct Project {
    git: bool,
    exclude: Option<Gitignore>,
    global: Option<Gitignore>,
}

impl Project {
    fn at(root: &Path, spent: &mut Spent) -> Self {
        let git = fs::symlink_metadata(root.join(".git")).ok();
        let git_directory = git.as_ref().is_some_and(|git| git.is_dir());
        let git = git_directory || git.is_some_and(|git| git.is_file());
        // `info` is a directory of the root's own `.git`, and no link either.
        let info = root.join(".git").join("info");
        let exclude = (git_directory
            && fs::symlink_metadata(&info).is_ok_and(|info| info.is_dir()))
        .then(|| patterns(root, &info.join("exclude"), Here, spent).ok())
        .flatten()
        .flatten();
        let global = git
            .then(ignore::gitignore::gitconfig_excludes_path)
            .flatten()
            .and_then(|path| patterns(root, &path, Followed, spent).ok().flatten());
        Self {
            git,
            exclude,
            global,
        }
    }

    /// Whether the entry at `path` under the directories `stack`, the root first, is ignored:
    /// the first kind of ignore file, in `rg`'s order, whose deepest match says anything decides.
    fn ignores(&self, stack: &[Directory], path: &Path, is_dir: bool) -> bool {
        let mut decided = Match::None;
        for kind in 0..IGNORE_FILES.len() {
            if kind == 2 && !self.git {
                break;
            }
            decided = stack
                .iter()
                .rev()
                .filter_map(|directory| directory.ignores[kind].as_ref())
                .map(|ignores| ignores.matched(path, is_dir))
                .find(|matched| !matched.is_none())
                .unwrap_or(Match::None);
            if !decided.is_none() {
                return decided.is_ignore();
            }
        }
        for ignores in [&self.exclude, &self.global].into_iter().flatten() {
            decided = ignores.matched(path, is_dir);
            if !decided.is_none() {
                break;
            }
        }
        decided.is_ignore()
    }
}

/// A directory being walked: what is left of its entries, and its ignore files.
struct Directory {
    path: PathBuf,
    entries: VecDeque<(OsString, FileType)>,
    /// Its `.rgignore`, `.ignore` and `.gitignore`, where it has them.
    ignores: [Option<Gitignore>; 3],
}

impl Directory {
    /// The directory at `path`, listed, with its ignore files read. `None` if it cannot be
    /// listed, or if it is `below_root` and holds a marker, as it is another project then. Its
    /// listing says what each entry is, so nothing more is looked at for the markers. Each entry
    /// read counts against `spent`, and `stop` is asked as they are read.
    fn open(
        path: &Path,
        project: &Project,
        below_root: bool,
        spent: &mut Spent,
        stop: &dyn Fn() -> bool,
    ) -> Result<Option<Self>, Ended> {
        let mut entries = VecDeque::new();
        let Ok(listing) = fs::read_dir(path) else {
            return Ok(None);
        };
        for entry in listing {
            if spent.entries == spent.limits.entries {
                return Err(Ended::Cut);
            }
            spent.entries += 1;
            if spent.entries.is_multiple_of(1024) && stop() {
                return Err(Ended::Stopped);
            }
            // An entry that cannot be read is left out, as `rg` leaves it.
            let Ok(entry) = entry else { continue };
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name();
            spent.names += name.len();
            if spent.names > spent.limits.bytes {
                return Err(Ended::Cut);
            }
            entries.push_back((name, kind));
        }
        if below_root && holds_marker(path, &entries) {
            return Ok(None);
        }
        let mut ignores = [None, None, None];
        for (ignores, name) in ignores.iter_mut().zip(IGNORE_FILES) {
            let here = entries
                .iter()
                .any(|(entry, kind)| entry == name && kind.is_file());
            let counts = name != ".gitignore" || project.git;
            if here && counts {
                *ignores = patterns(path, &path.join(name), Here, spent)?;
            }
        }
        Ok(Some(Self {
            path: path.to_path_buf(),
            entries,
            ignores,
        }))
    }
}

/// Whether the directory at `path`, whose listing is `entries`, holds a marker as detection
/// counts one (`project.rs`, whose rule this copies, as the two are on one layer): a `.git`
/// directory or file, a `.jj` or `.hg` directory, never a link, owned by this user or root. Only
/// an entry of a marker's name is looked at again, for its owner.
fn holds_marker(path: &Path, entries: &VecDeque<(OsString, FileType)>) -> bool {
    let [git, others @ ..] = MARKERS;
    entries.iter().any(|(name, kind)| {
        let named = (name == git && (kind.is_dir() || kind.is_file()))
            || (others.iter().any(|marker| name == *marker) && kind.is_dir());
        named && fs::symlink_metadata(path.join(name)).is_ok_and(|metadata| ours(&metadata))
    })
}

/// Whether this user or root owns what `metadata` describes, as detection asks.
fn ours(metadata: &fs::Metadata) -> bool {
    #[cfg(unix)]
    let by_root = std::os::unix::fs::MetadataExt::uid(metadata) == 0;
    #[cfg(not(unix))]
    let by_root = false;
    by_root || oso_platform::privacy::is_this_users(metadata)
}

/// How an ignore file is opened.
#[derive(Clone, Copy)]
enum Open {
    /// The project's own: never through a link.
    Here,
    /// The person's own global excludes, which may be a link to their dotfiles.
    Followed,
}
use Open::{Followed, Here};

/// The patterns of the ignore file at `file`, for the directory `base`: `None` unless it is a
/// regular file of at most [`MAX_IGNORE_FILE`] bytes, opened as `open` says, without waiting.
/// What it read counts against `spent`, and one longer than what is left of that ends the walk.
fn patterns(
    base: &Path,
    file: &Path,
    open: Open,
    spent: &mut Spent,
) -> Result<Option<Gitignore>, Ended> {
    let opened = match open {
        Here => oso_platform::file_meta::open_regular_here(file),
        Followed => oso_platform::file_meta::open_for_reading(file),
    };
    let Ok(Some((opened, metadata))) = opened else {
        return Ok(None);
    };
    if metadata.len() > MAX_IGNORE_FILE {
        return Ok(None);
    }
    let left = spent.limits.ignore_bytes - spent.ignore_bytes;
    if metadata.len() > left {
        return Err(Ended::Cut);
    }
    let mut bytes = Vec::new();
    // One byte past the bound tells a file that grew since it was looked at.
    let read = opened
        .take(MAX_IGNORE_FILE.min(left) + 1)
        .read_to_end(&mut bytes);
    if read.is_err() || bytes.len() as u64 > MAX_IGNORE_FILE.min(left) {
        return Ok(None);
    }
    spent.ignore_bytes += bytes.len() as u64;
    let mut builder = GitignoreBuilder::new(base);
    for line in bytes.split(|&byte| byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        // A line that is not UTF-8, or no pattern, is skipped, as `rg` skips it.
        if let Ok(line) = std::str::from_utf8(line) {
            let _ = builder.add_line(None, line);
        }
    }
    Ok(builder.build().ok().filter(|ignores| !ignores.is_empty()))
}

/// The path from `root` that the entry at `path`, of `kind`, is listed as: a file, or a link
/// that leads to one or leaves the root, with a UTF-8 path.
fn listed(
    root: &Path,
    path: &Path,
    kind: FileType,
    spent: &mut Spent,
) -> Result<Option<Box<str>>, Ended> {
    let file = kind.is_file()
        || (kind.is_symlink() && {
            let mut looks = spent.limits.entries - spent.entries;
            let leads = resolve(root, path, &mut looks);
            spent.entries = spent.limits.entries - looks;
            match leads {
                Leads::Inside(file) => file,
                Leads::Outside => true,
                Leads::Nowhere => false,
                Leads::Spent => return Err(Ended::Cut),
            }
        });
    if !file {
        return Ok(None);
    }
    let Some(relative) = path.strip_prefix(root).ok().and_then(Path::to_str) else {
        return Ok(None);
    };
    // One spelling on every system: `/` is no name's on Windows either.
    #[cfg(windows)]
    let relative = relative.replace('\\', "/");
    Ok(Some(relative.into()))
}

mod links;

use links::{Leads, resolve};

#[cfg(test)]
mod tests;
