//! The directories a walk is in, open, and what each says of its entries: its ignore files, and
//! whether it holds a marker (ADR 0114).
//!
//! Must not depend on: `app`, winit or the UI thread; this touches the file system.
//!
//! Each directory below the root is opened from its parent by its name, following no link, and
//! listed from what was opened. At most [`MAX_HELD`] stay open, the root and the deepest; one
//! let go is opened again from the nearest one held, name by name in the same way, when its
//! entries are walked again, and only if it is still the directory it was, by its device and
//! inode. Each name opened again counts as an entry read.

use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::fs::{File, Metadata};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ignore::Match;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use oso_core::project::MARKERS;
use oso_platform::file_meta::{Directory, EntryKind};

use super::{Ended, IGNORE_FILES, MAX_IGNORE_FILE, Spent};

/// The most directories a walk holds open, as an open directory is a descriptor and a process
/// may hold only a few hundred.
pub(super) const MAX_HELD: usize = 32;

/// What applies to a whole project: whether it is a git repository, whose `.gitignore` files
/// count, and its root's `info/exclude` and the person's global excludes, which apply only
/// then.
pub(super) struct Project {
    git: bool,
    exclude: Option<Gitignore>,
    global: Option<Gitignore>,
}

impl Project {
    /// The project at `root`, which `opened` is. An exclude file past what is left of the
    /// bytes of ignore files gives no patterns, and the walk goes on.
    pub(super) fn at(root: &Path, opened: &Directory, spent: &mut Spent) -> Self {
        let git = opened.kind_of(OsStr::new(".git")).ok();
        let git_directory = git == Some(EntryKind::Directory);
        let git = git_directory || git == Some(EntryKind::File);
        // `info` is a directory of the root's own `.git`, and no link either.
        let exclude = git_directory
            .then(|| opened.open_directory(OsStr::new(".git")).ok().flatten())
            .flatten()
            .and_then(|git| git.open_directory(OsStr::new("info")).ok().flatten())
            .and_then(|info| {
                let file = info.open_file(OsStr::new("exclude"));
                patterns(root, file, spent).ok().flatten()
            });
        // The person's own, which may be a link to their dotfiles.
        let global = git
            .then(ignore::gitignore::gitconfig_excludes_path)
            .flatten()
            .and_then(|path| {
                let file = oso_platform::file_meta::open_for_reading(&path);
                patterns(root, file, spent).ok().flatten()
            });
        Self {
            git,
            exclude,
            global,
        }
    }

    /// Whether the entry at `path` under the directories `stack`, the root first, is ignored:
    /// the first kind of ignore file, in `rg`'s order, whose deepest match says anything decides.
    pub(super) fn ignores(&self, stack: &[Listed], path: &Path, is_dir: bool) -> bool {
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
pub(super) struct Listed {
    /// Its path, the root's joined with the names on the way, which ignore files match.
    pub(super) path: PathBuf,
    /// Its name in its parent, which it is opened again by.
    name: OsString,
    /// Its device and inode, where the system has them, taken as it is let go.
    identity: Option<(u64, u64)>,
    /// It, open, unless it was let go.
    opened: Option<Arc<Directory>>,
    pub(super) entries: VecDeque<(OsString, EntryKind)>,
    /// Its `.rgignore`, `.ignore` and `.gitignore`, where it has them.
    ignores: [Option<Gitignore>; 3],
}

impl Listed {
    /// The root, `opened` at `path`, listed, with its ignore files read. `None` if it cannot be
    /// listed.
    pub(super) fn open(
        opened: Arc<Directory>,
        path: PathBuf,
        project: &Project,
        spent: &mut Spent,
        stop: &dyn Fn() -> bool,
    ) -> Result<Option<Self>, Ended> {
        Self::list(opened, path, OsString::new(), false, project, spent, stop)
    }

    /// The directory `name` below the root, `opened` at `path`, as [`Listed::open`] has it, and
    /// `None` too if it holds a marker, as it is another project then.
    pub(super) fn below(
        opened: Arc<Directory>,
        path: PathBuf,
        name: OsString,
        project: &Project,
        spent: &mut Spent,
        stop: &dyn Fn() -> bool,
    ) -> Result<Option<Self>, Ended> {
        Self::list(opened, path, name, true, project, spent, stop)
    }

    /// Its listing says what each entry is, so nothing more is looked at for the markers. Each
    /// entry read counts against `spent`, and `stop` is asked as they are read.
    fn list(
        opened: Arc<Directory>,
        path: PathBuf,
        name: OsString,
        below_root: bool,
        project: &Project,
        spent: &mut Spent,
        stop: &dyn Fn() -> bool,
    ) -> Result<Option<Self>, Ended> {
        let mut entries = VecDeque::new();
        let listing = match opened.entries() {
            Ok(listing) => listing,
            Err(error) if expected(&error) => return Ok(None),
            Err(_) => return Err(Ended::Cut),
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
            spent.names += entry.name.len();
            if spent.names > spent.limits.bytes {
                return Err(Ended::Cut);
            }
            entries.push_back((entry.name, entry.kind));
        }
        if below_root && holds_marker(&opened, &entries) {
            return Ok(None);
        }
        let mut ignores = [None, None, None];
        for (ignores, name) in ignores.iter_mut().zip(IGNORE_FILES) {
            let here = entries
                .iter()
                .any(|(entry, kind)| entry == name && *kind == EntryKind::File);
            let counts = name != ".gitignore" || project.git;
            if here && counts {
                *ignores = patterns(&path, opened.open_file(OsStr::new(name)), spent)?;
            }
        }
        Ok(Some(Self {
            path,
            name,
            identity: None,
            opened: Some(opened),
            entries,
            ignores,
        }))
    }
}

/// Whether `error`, opening or listing a directory, says only that it is not there to walk:
/// gone, refused, or no directory. Any other, as running out of descriptors, ends a walk cut,
/// so that it never says it walked everything.
pub(super) fn expected(error: &io::Error) -> bool {
    // A link or no directory at the name opens as none already.
    matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied | io::ErrorKind::NotADirectory
    )
}

/// The device and inode of `directory`, where the system has them.
fn identity(directory: &Directory) -> Option<(u64, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        directory
            .metadata()
            .ok()
            .map(|metadata| (metadata.dev(), metadata.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = directory;
        None
    }
}

/// The directories a walk is in, the root first, of which at most [`MAX_HELD`] are open.
pub(super) struct Stack {
    listed: Vec<Listed>,
}

impl Stack {
    pub(super) fn new() -> Self {
        Self { listed: Vec::new() }
    }

    /// Goes into `listed`, letting go of the shallowest held but the root past [`MAX_HELD`].
    pub(super) fn push(&mut self, listed: Listed) {
        self.listed.push(listed);
        let mut held = self.listed.iter().filter(|l| l.opened.is_some()).count();
        for listed in self.listed.iter_mut().skip(1) {
            if held <= MAX_HELD {
                break;
            }
            if let Some(opened) = listed.opened.take() {
                listed.identity = identity(&opened);
                held -= 1;
            }
        }
    }

    pub(super) fn pop(&mut self) {
        self.listed.pop();
    }

    pub(super) fn last_mut(&mut self) -> Option<&mut Listed> {
        self.listed.last_mut()
    }

    pub(super) fn listed(&self) -> &[Listed] {
        &self.listed
    }

    /// The root, which is always held.
    pub(super) fn root(&self) -> &Directory {
        self.listed
            .first()
            .and_then(|root| root.opened.as_deref())
            .expect("the root is held while it is walked")
    }

    /// The deepest directory, open: opened again if it was let go, from the nearest one held,
    /// name by name, without following a link, each name an entry read. `None` if it is not the
    /// directory it was any more, whose entries are then left.
    pub(super) fn open_top(&mut self, spent: &mut Spent) -> Result<Option<Arc<Directory>>, Ended> {
        let Some(top) = self.listed.len().checked_sub(1) else {
            return Ok(None);
        };
        if let Some(opened) = &self.listed[top].opened {
            return Ok(Some(Arc::clone(opened)));
        }
        let held = (0..top)
            .rev()
            .find(|&index| self.listed[index].opened.is_some())
            .unwrap_or(0);
        let Some(from) = self.listed[held].opened.clone() else {
            return Ok(None);
        };
        let mut at = from;
        for listed in &self.listed[held + 1..=top] {
            if spent.entries == spent.limits.entries {
                return Err(Ended::Cut);
            }
            spent.entries += 1;
            let again = match at.open_directory(&listed.name) {
                Ok(again) => again,
                Err(error) if expected(&error) => None,
                Err(_) => return Err(Ended::Cut),
            };
            // Each one on the way was let go, so its identity was taken then.
            let again = again.filter(|again| identity(again) == listed.identity);
            match again {
                Some(again) => at = Arc::new(again),
                None => {
                    self.listed[top].entries.clear();
                    return Ok(None);
                }
            }
        }
        self.listed[top].opened = Some(Arc::clone(&at));
        Ok(Some(at))
    }

    /// How many directories are held open, for a test.
    #[cfg(test)]
    pub(super) fn held(&self) -> usize {
        self.listed.iter().filter(|l| l.opened.is_some()).count()
    }
}

/// Whether the directory `opened`, whose listing is `entries`, holds a marker as detection
/// counts one (`project.rs`, whose rule this copies, as the two are on one layer): a `.git`
/// directory or file, a `.jj` or `.hg` directory, never a link, owned by this user or root. Only
/// an entry of a marker's name is opened, from the directory, for its owner.
fn holds_marker(opened: &Directory, entries: &VecDeque<(OsString, EntryKind)>) -> bool {
    let [git, others @ ..] = MARKERS;
    entries.iter().any(|(name, kind)| {
        let directory = (name == git || others.iter().any(|marker| name == *marker))
            && *kind == EntryKind::Directory;
        if directory {
            return opened
                .open_directory(name)
                .ok()
                .flatten()
                .and_then(|marker| marker.metadata().ok())
                .is_some_and(|metadata| ours(&metadata));
        }
        name == git
            && *kind == EntryKind::File
            && opened
                .open_file(name)
                .ok()
                .flatten()
                .is_some_and(|(_, metadata)| ours(&metadata))
    })
}

/// Whether this user or root owns what `metadata` describes, as detection asks.
fn ours(metadata: &Metadata) -> bool {
    #[cfg(unix)]
    let by_root = std::os::unix::fs::MetadataExt::uid(metadata) == 0;
    #[cfg(not(unix))]
    let by_root = false;
    by_root || oso_platform::privacy::is_this_users(metadata)
}

/// The patterns of the ignore file `opened`, for the directory `base`: `None` unless it is a
/// regular file of at most [`MAX_IGNORE_FILE`] bytes, opened without waiting. What it read
/// counts against `spent`, and one longer than what is left of that ends the walk.
fn patterns(
    base: &Path,
    opened: io::Result<Option<(File, Metadata)>>,
    spent: &mut Spent,
) -> Result<Option<Gitignore>, Ended> {
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
