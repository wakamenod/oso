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
//! - At most [`MAX_WALKED`] files are listed, and at most [`MAX_PATH_BYTES`] of their paths;
//!   past either the walk stops and says it was cut.
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
use std::path::{Component, Path, PathBuf};
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

/// How much a walk lists at most.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub files: usize,
    pub bytes: usize,
}

impl Limits {
    /// A project's: [`MAX_WALKED`] files, and [`MAX_PATH_BYTES`] of their paths (ADR 0114).
    pub const PROJECT: Self = Self {
        files: MAX_WALKED,
        bytes: MAX_PATH_BYTES,
    };
}

/// The most links a link's path goes through before it is taken for one that leads nowhere, as
/// the system's own bound.
const MAX_HOPS: usize = 40;

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
}

/// Hands a batch to the event loop; `false` once it is gone.
type SendBatch = std::sync::Arc<dyn Fn(Walked) -> bool + Send + Sync>;

impl Walker {
    /// A walker that hands each batch to `send`.
    pub fn new(send: impl Fn(Walked) -> bool + Send + Sync + 'static) -> Self {
        Self {
            requests: None,
            send: std::sync::Arc::new(send),
        }
    }

    /// Walks `request`'s root on the walker's thread, after any walk before it, which a newer
    /// listing has stopped. If the thread cannot start, the walk ends at once with nothing
    /// found, so the prompt stops waiting.
    pub fn walk(&mut self, request: WalkRequest) {
        if self.requests.is_none() {
            let (requests, queue) = mpsc::channel::<WalkRequest>();
            let send = std::sync::Arc::clone(&self.send);
            let spawned = thread::Builder::new()
                .name("oso-walk".into())
                .spawn(move || serve(&queue, &*send));
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
fn serve(queue: &mpsc::Receiver<WalkRequest>, send: &(dyn Fn(Walked) -> bool + Send + Sync)) {
    while let Ok(request) = queue.recv() {
        let generation = request.generation;
        let live = || request.live.load(Ordering::Relaxed) == generation;
        if !live() {
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
    let project = Project::at(root);
    let mut stack = Vec::new();
    if let Some(directory) = Directory::open(root, &project, false) {
        stack.push(directory);
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
            if let Some(directory) = Directory::open(&path, &project, true) {
                stack.push(directory);
            }
        } else if let Some(path) = listed(root, &path, kind) {
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
    fn at(root: &Path) -> Self {
        let git = fs::symlink_metadata(root.join(".git")).ok();
        let git_directory = git.as_ref().is_some_and(|git| git.is_dir());
        let git = git_directory || git.is_some_and(|git| git.is_file());
        // `info` is a directory of the root's own `.git`, and no link either.
        let info = root.join(".git").join("info");
        let exclude = (git_directory
            && fs::symlink_metadata(&info).is_ok_and(|info| info.is_dir()))
        .then(|| patterns(root, &info.join("exclude"), Here))
        .flatten();
        let global = git
            .then(ignore::gitignore::gitconfig_excludes_path)
            .flatten()
            .and_then(|path| patterns(root, &path, Followed));
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
    /// listing says what each entry is, so nothing more is looked at for the markers.
    fn open(path: &Path, project: &Project, below_root: bool) -> Option<Self> {
        let mut entries = VecDeque::new();
        for entry in fs::read_dir(path).ok()? {
            // An entry that cannot be read is left out, as `rg` leaves it.
            let Ok(entry) = entry else { continue };
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            entries.push_back((entry.file_name(), kind));
        }
        if below_root && holds_marker(path, &entries) {
            return None;
        }
        let ignores = IGNORE_FILES.map(|name| {
            let here = entries
                .iter()
                .any(|(entry, kind)| entry == name && kind.is_file());
            let counts = name != ".gitignore" || project.git;
            (here && counts)
                .then(|| patterns(path, &path.join(name), Here))
                .flatten()
        });
        Some(Self {
            path: path.to_path_buf(),
            entries,
            ignores,
        })
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
fn patterns(base: &Path, file: &Path, open: Open) -> Option<Gitignore> {
    let opened = match open {
        Here => oso_platform::file_meta::open_regular_here(file),
        Followed => oso_platform::file_meta::open_for_reading(file),
    };
    let (opened, metadata) = opened.ok()??;
    if metadata.len() > MAX_IGNORE_FILE {
        return None;
    }
    let mut bytes = Vec::new();
    // One byte past the bound tells a file that grew since it was looked at.
    opened
        .take(MAX_IGNORE_FILE + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_IGNORE_FILE {
        return None;
    }
    let mut builder = GitignoreBuilder::new(base);
    for line in bytes.split(|&byte| byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        // A line that is not UTF-8, or no pattern, is skipped, as `rg` skips it.
        if let Ok(line) = std::str::from_utf8(line) {
            let _ = builder.add_line(None, line);
        }
    }
    builder.build().ok().filter(|ignores| !ignores.is_empty())
}

/// The path from `root` that the entry at `path`, of `kind`, is listed as: a file, or a link
/// that leads to one or leaves the root, with a UTF-8 path.
fn listed(root: &Path, path: &Path, kind: FileType) -> Option<Box<str>> {
    let file = kind.is_file()
        || (kind.is_symlink()
            && match resolve(root, path) {
                Leads::Inside(file) => file,
                Leads::Outside => true,
                Leads::Nowhere => false,
            });
    if !file {
        return None;
    }
    let relative = path.strip_prefix(root).ok()?.to_str()?;
    // One spelling on every system: `/` is no name's on Windows either.
    #[cfg(windows)]
    let relative = relative.replace('\\', "/");
    Some(relative.into())
}

/// Where a link leads, found by its words.
#[derive(Debug, PartialEq, Eq)]
enum Leads {
    /// Under the root, to a file (`true`) or to something else.
    Inside(bool),
    /// Out of the root, at some hop: nothing there is looked at.
    Outside,
    /// Nowhere: a link of too many hops, or one that cannot be read.
    Nowhere,
}

/// Where the link at `link`, under `root`, leads. Each name on the way is looked at without
/// following it, and each link's target is read and taken by its words, only while the way
/// stays under `root`.
fn resolve(root: &Path, link: &Path) -> Leads {
    let Ok(relative) = link.strip_prefix(root) else {
        return Leads::Outside;
    };
    let mut ahead: VecDeque<OsString> = VecDeque::new();
    let mut at = root.to_path_buf();
    if !push_ahead(&mut ahead, relative) {
        return Leads::Outside;
    }
    let mut hops = 0;
    while let Some(name) = ahead.pop_front() {
        if name == ".." {
            if at == root {
                return Leads::Outside;
            }
            at.pop();
            continue;
        }
        let next = at.join(&name);
        let Ok(metadata) = fs::symlink_metadata(&next) else {
            return Leads::Nowhere;
        };
        if !metadata.is_symlink() {
            if ahead.is_empty() {
                return Leads::Inside(metadata.is_file());
            }
            at = next;
            continue;
        }
        hops += 1;
        let Ok(target) = fs::read_link(&next) else {
            return Leads::Nowhere;
        };
        if hops > MAX_HOPS {
            return Leads::Nowhere;
        }
        let mut rest = VecDeque::new();
        let target = if target.has_root() {
            // Taken from the root again, if it names a place under it by its words.
            match target.strip_prefix(root) {
                Ok(under) => {
                    at = root.to_path_buf();
                    under.to_path_buf()
                }
                Err(_) => return Leads::Outside,
            }
        } else {
            target
        };
        if !push_ahead(&mut rest, &target) {
            return Leads::Outside;
        }
        rest.extend(ahead);
        ahead = rest;
    }
    Leads::Inside(false)
}

/// Puts the names of `path`, relative, at the end of `ahead`: `..` as one, `.` as none. `false`
/// for a path with a root or a prefix, which leaves the place it is taken from.
fn push_ahead(ahead: &mut VecDeque<OsString>, path: &Path) -> bool {
    for component in path.components() {
        match component {
            Component::Normal(name) => ahead.push_back(name.to_os_string()),
            Component::ParentDir => ahead.push_back(OsString::from("..")),
            Component::CurDir => {}
            Component::RootDir | Component::Prefix(_) => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests;
