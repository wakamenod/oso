//! `.worktreeinclude` (ADR 0121, Q16): its patterns, read from a repository's main checkout, and
//! the files they match among those git ignores, copied into a new worktree as Claude Code copies
//! them. Only regular files are copied, no link is followed on either side, nothing outside the
//! two checkouts is touched, and the copy stops at fixed caps. Git's answer comes from the
//! caller: this runs no process.
//!
//! Must not depend on: `app`, `dispatch`, `commands` or the UI thread; it touches the disk, on a
//! worktree job's thread.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::{File, Metadata};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use oso_platform::file_meta::{Directory, Entry, EntryKind, open_regular_here};
use oso_platform::privacy::has_one_name;

use super::porcelain::printable;

/// The file's name, at the root of the main checkout.
pub const NAME: &str = ".worktreeinclude";
/// The most bytes of `.worktreeinclude` read.
pub const MAX_PATTERN_BYTES: u64 = 64 * 1024;
/// The most patterns it may hold, not counting blank lines and comments.
pub const MAX_PATTERNS: usize = 1_000;
/// The most files copied into one worktree.
pub const MAX_FILES: usize = 1_000;
/// The most bytes copied into one worktree.
pub const MAX_BYTES: u64 = 256 * 1024 * 1024;
/// Entries looked at while descending ignored directories git listed whole.
pub const MAX_VISITS: usize = 100_000;
/// The deepest a listed directory is descended, so that the directories held open stay few.
const MAX_DEPTH: usize = 64;

/// `.worktreeinclude`'s patterns, compiled.
#[derive(Debug)]
pub struct Patterns {
    ignore: Gitignore,
}

impl Patterns {
    /// Whether the patterns take the path `relative` to the checkout's root, or a directory it
    /// is in.
    fn take(&self, relative: &Path, is_dir: bool) -> bool {
        self.ignore
            .matched_path_or_any_parents(relative, is_dir)
            .is_ignore()
    }
}

/// Why `.worktreeinclude` could not be used.
#[derive(Debug)]
pub enum IncludeError {
    /// It is over [`MAX_PATTERN_BYTES`].
    TooLarge,
    /// It holds over [`MAX_PATTERNS`] patterns.
    TooMany,
    /// It could not be opened or read.
    Read(io::Error),
    /// A pattern is not valid, as the `ignore` crate says.
    Pattern(String),
}

impl fmt::Display for IncludeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge => write!(f, "{NAME} is larger than 64 KiB"),
            Self::TooMany => write!(f, "{NAME} has more than {MAX_PATTERNS} patterns"),
            Self::Read(error) => write!(f, "{NAME} could not be read: {error}"),
            Self::Pattern(error) => write!(
                f,
                "{NAME} has a pattern that is not valid: {}",
                printable(error, 200)
            ),
        }
    }
}

impl std::error::Error for IncludeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read(error) => Some(error),
            _ => None,
        }
    }
}

/// Reads `root/.worktreeinclude` and compiles its patterns: `None` if nothing is there, or if
/// it is no regular file (a link, a directory, a device), as then there is nothing to copy. A
/// line that is not UTF-8 is skipped. It touches the disk, so it belongs off the UI thread.
///
/// # Errors
///
/// If it is over [`MAX_PATTERN_BYTES`] or [`MAX_PATTERNS`], cannot be read, or holds a pattern
/// that is not valid.
pub fn read(root: &Path) -> Result<Option<Patterns>, IncludeError> {
    let (file, metadata) = match open_regular_here(&root.join(NAME)) {
        Ok(Some(opened)) => opened,
        Ok(None) => return Ok(None),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(IncludeError::Read(error)),
    };
    if metadata.len() > MAX_PATTERN_BYTES {
        return Err(IncludeError::TooLarge);
    }
    let mut bytes = Vec::new();
    file.take(MAX_PATTERN_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(IncludeError::Read)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_PATTERN_BYTES {
        return Err(IncludeError::TooLarge);
    }
    compile(root, &bytes).map(Some)
}

/// The patterns `bytes` hold, one a line, matched relative to `root`.
fn compile(root: &Path, bytes: &[u8]) -> Result<Patterns, IncludeError> {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let mut builder = GitignoreBuilder::new(root);
    let mut count = 0;
    for line in bytes.split(|&byte| byte == b'\n') {
        let Ok(line) = std::str::from_utf8(line) else {
            continue;
        };
        let pattern = line.trim_end();
        if pattern.is_empty() || pattern.starts_with('#') {
            continue;
        }
        count += 1;
        if count > MAX_PATTERNS {
            return Err(IncludeError::TooMany);
        }
        builder
            .add_line(None, line)
            .map_err(|error| IncludeError::Pattern(error.to_string()))?;
    }
    let ignore = builder
        .build()
        .map_err(|error| IncludeError::Pattern(error.to_string()))?;
    Ok(Patterns { ignore })
}

/// What was copied, and why it stopped early, if it did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Copied {
    pub files: usize,
    pub bytes: u64,
    pub stopped: Option<Stop>,
}

/// The cap a copy stopped at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// [`MAX_FILES`].
    Files,
    /// [`MAX_BYTES`].
    Bytes,
    /// [`MAX_VISITS`].
    Visits,
}

/// The caps of one copy.
#[derive(Debug, Clone, Copy)]
struct Limits {
    files: usize,
    bytes: u64,
    visits: usize,
}

/// Copies into `to`, the new worktree's root, the files under `from`, the main checkout's root,
/// that `patterns` take among `ignored`: the paths `git ls-files -z --others --ignored
/// --exclude-standard --directory` gave, relative to `from`, a directory ending with `/`.
///
/// A listed file is copied if the patterns take it. A listed directory is copied whole if they
/// take it as a directory; otherwise it is descended and each regular file in it they take is
/// copied, after every listed file and every directory taken whole, so that a large directory
/// cannot use the visits up before them. Each directory descended gets an equal share of the
/// visits left, so one large directory cannot use them all either. Descending follows no link and skips a nested
/// repository. A listed path with a `..` or `.git` component or a root, or that leads through a
/// link, is skipped. On Unix a file with more than one name is skipped. Into `to`, no link is
/// written through: a file whose directory there is a link or no directory is skipped, and a
/// file already there is left as it is. Each file keeps
/// its permission bits, but set-user-ID, set-group-ID and sticky. A file that cannot be copied is
/// skipped. It touches the disk, so it belongs on a worker thread.
///
/// # Errors
///
/// If `from` or `to` cannot be opened as a directory.
pub fn copy(
    patterns: &Patterns,
    ignored: &[PathBuf],
    from: &Path,
    to: &Path,
) -> io::Result<Copied> {
    let limits = Limits {
        files: MAX_FILES,
        bytes: MAX_BYTES,
        visits: MAX_VISITS,
    };
    copy_within(patterns, ignored, from, to, limits)
}

/// [`copy`], within `limits`.
fn copy_within(
    patterns: &Patterns,
    ignored: &[PathBuf],
    from: &Path,
    to: &Path,
    limits: Limits,
) -> io::Result<Copied> {
    let source = Directory::open(from)?;
    let mut copier = Copier {
        patterns,
        to_root: to,
        to: Directory::open(to)?,
        limits,
        visits: 0,
        share_end: limits.visits,
        share_cut: false,
        copied: Copied::default(),
    };
    let mut descended = Vec::new();
    for listed in ignored {
        let Some(names) = names_of(listed) else {
            continue;
        };
        let is_dir = listed.as_os_str().as_encoded_bytes().ends_with(b"/");
        if is_dir && !patterns.take(&joined(&names), true) {
            descended.push(names);
        } else if copier.copied.stopped.is_none() {
            copier.listed(&source, names, is_dir);
        }
    }
    let count = descended.len();
    for (done, names) in descended.into_iter().enumerate() {
        if copier.copied.stopped.is_some() {
            break;
        }
        let left = limits.visits.saturating_sub(copier.visits);
        copier.share_end = copier.visits + left / (count - done);
        copier.listed(&source, names, true);
    }
    if copier.share_cut {
        copier.copied.stopped.get_or_insert(Stop::Visits);
    }
    Ok(copier.copied)
}

/// The names of `listed`'s components: `None` if it has a root, a `..` or a `.git`, or is empty.
fn names_of(listed: &Path) -> Option<Vec<OsString>> {
    let mut names = Vec::new();
    for component in listed.components() {
        match component {
            Component::Normal(name) if !name.eq_ignore_ascii_case(".git") => {
                names.push(name.to_os_string());
            }
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!names.is_empty()).then_some(names)
}

/// The relative path `names` make.
fn joined(names: &[OsString]) -> PathBuf {
    names.iter().collect()
}

/// Calls `then` with the directory `names` lead to from `root`, if each of them is a directory
/// and none a link.
fn below(root: &Directory, names: &[OsString], then: impl FnOnce(&Directory)) {
    let mut current: Option<Directory> = None;
    for name in names {
        match current.as_ref().unwrap_or(root).open_directory(name) {
            Ok(Some(next)) => current = Some(next),
            Ok(None) => return,
            Err(error) => {
                log::debug!("worktree include: a listed directory: {}", error.kind());
                return;
            }
        }
    }
    then(current.as_ref().unwrap_or(root));
}

/// One copy under way.
struct Copier<'a> {
    patterns: &'a Patterns,
    /// The new worktree's root, only to remove a file left partly written.
    to_root: &'a Path,
    to: Directory,
    limits: Limits,
    visits: usize,
    /// The visits after which the directory being descended stops: its share.
    share_end: usize,
    /// A directory stopped at its share.
    share_cut: bool,
    copied: Copied,
}

impl Copier<'_> {
    /// Copies the listed path `names`, a directory if `is_dir`.
    fn listed(&mut self, source: &Directory, mut names: Vec<OsString>, is_dir: bool) {
        if is_dir {
            let whole = self.patterns.take(&joined(&names), true);
            let path = names.clone();
            below(source, &path, |directory| {
                self.descend(directory, &mut names, whole, 0);
            });
        } else if self.patterns.take(&joined(&names), false) {
            let parents = names.split_last().map_or(&[][..], |(_, parents)| parents);
            below(source, parents, |directory| self.file(directory, &names));
        }
    }

    /// Copies what `directory`, at `names`, holds: each regular file if `whole`, otherwise each
    /// one the patterns take, and the same below it. A nested repository is skipped.
    fn descend(
        &mut self,
        directory: &Directory,
        names: &mut Vec<OsString>,
        whole: bool,
        depth: usize,
    ) {
        if depth > MAX_DEPTH {
            log::debug!("worktree include: a directory deeper than {MAX_DEPTH} levels skipped");
            return;
        }
        if holds_git(directory) {
            return;
        }
        let Some(entries) = self.entries(directory) else {
            return;
        };
        for entry in entries {
            if self.copied.stopped.is_some() || self.visits > self.share_end {
                return;
            }
            names.push(entry.name);
            let name = names.last().map_or(OsStr::new(""), OsString::as_os_str);
            match entry.kind {
                EntryKind::File if whole || self.patterns.take(&joined(names), false) => {
                    self.file(directory, names);
                }
                EntryKind::Directory => match directory.open_directory(name) {
                    Ok(Some(below)) => self.descend(&below, names, whole, depth + 1),
                    Ok(None) => {}
                    Err(error) => log::debug!("worktree include: a directory: {}", error.kind()),
                },
                _ => {}
            }
            names.pop();
        }
    }

    /// The entries of `directory`, each counted as a visit: `None` past the cap, which stops the
    /// copy. A listing that fails ends where it failed.
    fn entries(&mut self, directory: &Directory) -> Option<Vec<Entry>> {
        let listing = match directory.entries() {
            Ok(listing) => listing,
            Err(error) => {
                log::debug!("worktree include: a listing: {}", error.kind());
                return Some(Vec::new());
            }
        };
        let mut entries = Vec::new();
        for entry in listing {
            self.visits += 1;
            if self.visits > self.limits.visits {
                self.copied.stopped = Some(Stop::Visits);
                return None;
            }
            if self.visits > self.share_end {
                self.share_cut = true;
                return None;
            }
            match entry {
                Ok(entry) => entries.push(entry),
                Err(error) => log::debug!("worktree include: an entry: {}", error.kind()),
            }
        }
        Some(entries)
    }

    /// Copies the regular file `names` names, in `directory`, into the same place in the new
    /// worktree, unless something is there already.
    fn file(&mut self, directory: &Directory, names: &[OsString]) {
        let Some((name, parents)) = names.split_last() else {
            return;
        };
        let (source, metadata) = match directory.open_file(name) {
            Ok(Some(opened)) => opened,
            Ok(None) => return,
            Err(error) => {
                log::debug!("worktree include: a source file: {}", error.kind());
                return;
            }
        };
        // Only a file with one name is copied.
        if !has_one_name(&metadata) {
            log::debug!("worktree include: a file with more than one name skipped");
            return;
        }
        if self.copied.files >= self.limits.files {
            self.copied.stopped = Some(Stop::Files);
            return;
        }
        let remaining = self.limits.bytes.saturating_sub(self.copied.bytes);
        if metadata.len() > remaining {
            self.copied.stopped = Some(Stop::Bytes);
            return;
        }
        let Some(mut target) = self.target(parents, name, mode_of(&metadata)) else {
            return;
        };
        // One byte past what is left tells a file that grew past the cap.
        match io::copy(
            &mut (&source).take(remaining.saturating_add(1)),
            &mut target,
        ) {
            Ok(written) if written <= remaining => {
                self.copied.files += 1;
                self.copied.bytes += written;
            }
            Ok(_) => {
                self.remove(names, target);
                self.copied.stopped = Some(Stop::Bytes);
            }
            Err(error) => {
                log::debug!("worktree include: copying a file: {}", error.kind());
                self.remove(names, target);
            }
        }
    }

    /// A new file `name` in the new worktree, below the directories `parents`, each made if
    /// missing: `None` if one of them is a link or no directory, or something is at `name`.
    fn target(&self, parents: &[OsString], name: &OsStr, mode: u32) -> Option<File> {
        let mut current: Option<Directory> = None;
        for parent in parents {
            match current
                .as_ref()
                .unwrap_or(&self.to)
                .create_directory(parent)
            {
                Ok(Some(next)) => current = Some(next),
                Ok(None) => {
                    log::debug!("worktree include: a link or a file is in a target's way");
                    return None;
                }
                Err(error) => {
                    log::debug!("worktree include: a target directory: {}", error.kind());
                    return None;
                }
            }
        }
        match current.as_ref().unwrap_or(&self.to).create_file(name, mode) {
            Ok(file) => Some(file),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => None,
            Err(error) => {
                log::debug!("worktree include: a target file: {}", error.kind());
                None
            }
        }
    }

    /// Removes the file `names` in the new worktree, left partly written as `file`: emptied
    /// first, and removed by its path only if that path is still the file.
    fn remove(&self, names: &[OsString], file: File) {
        if let Err(error) = file.set_len(0) {
            log::debug!(
                "worktree include: emptying a partial file: {}",
                error.kind()
            );
        }
        let path = self.to_root.join(joined(names));
        if !same_file(&path, &file) {
            return;
        }
        drop(file);
        if let Err(error) = std::fs::remove_file(&path) {
            log::debug!(
                "worktree include: removing a partial file: {}",
                error.kind()
            );
        }
    }
}

/// Whether `directory` holds a `.git` entry, as a nested repository does. One that cannot be
/// looked at counts as one.
fn holds_git(directory: &Directory) -> bool {
    match directory.kind_of(OsStr::new(".git")) {
        Ok(_) => true,
        Err(error) => error.kind() != io::ErrorKind::NotFound,
    }
}

/// The permission bits a copy of the file `metadata` describes gets: set-user-ID, set-group-ID
/// and sticky left out.
#[cfg(unix)]
fn mode_of(metadata: &Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o777
}

#[cfg(not(unix))]
fn mode_of(_: &Metadata) -> u32 {
    0o644
}

/// Whether `path`, not followed, is the open `file`.
#[cfg(unix)]
fn same_file(path: &Path, file: &File) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::symlink_metadata(path), file.metadata()) {
        (Ok(there), Ok(open)) => there.dev() == open.dev() && there.ino() == open.ino(),
        _ => false,
    }
}

#[cfg(not(unix))]
fn same_file(path: &Path, _: &File) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|there| there.is_file())
}

#[cfg(all(test, unix))]
mod tests;
