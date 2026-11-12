//! Projects (ADR 0114): what the host found of a file's project, read as plain data, and what
//! the editor keeps of each.
//!
//! Must not depend on: `buffer`, `editor` or file I/O. These are pure functions of names and
//! bytes; the host looks at the disk, on a thread of its own, and runs nothing.
//!
//! A git worktree's `.git` is a file naming `<main>/.git/worktrees/<name>`. What it names is
//! kept here by its words alone, never followed: it counts for trust only once the main
//! repository it names is trusted and records this worktree back, which the host checks.
//!
//! The record of trusted projects is [`TrustList`], and what a search of a project looks for
//! is [`LineQuery`].

use std::path::{Component, Path, PathBuf};

mod search;
mod trust;

pub use search::{BLOCK, LineHit, LineQuery, MAX_LINE, MAX_ROW_BYTES, MAX_ROWS, row_text};
pub use trust::{NotRecordable, TrustChange, TrustList};

/// The most bytes of a `.git` file that are read. Git writes one short line in it.
pub const MAX_GIT_FILE: u64 = 1024;

/// The names whose presence makes a directory a project's root: a version-control marker,
/// `.git` a directory or a file, `.jj` and `.hg` directories (ADR 0114). A walk of a project
/// never enters them, nor a directory holding one, which is another project.
pub const MARKERS: [&str; 3] = [".git", ".jj", ".hg"];

/// The most files a walk of a project lists or searches; past them it stops, and the count
/// says `+` (ADR 0114).
pub const MAX_WALKED: usize = 100_000;

/// The main repository a worktree's `.git` file names, if it names one: its first line is
/// `gitdir: <path>`, and the path, taken relative to `root` when it is relative and with `.`
/// and `..` worked out by its words, has the shape `<main>/.git/worktrees/<name>`. Nothing
/// is looked up on disk. `None` for anything else, a submodule's `.git/modules/…` among them.
pub fn worktree_claim(root: &Path, git_file: &[u8]) -> Option<PathBuf> {
    let gitdir = worktree_entry(root, git_file)?;
    worktree_main(&gitdir).map(Path::to_path_buf)
}

/// The worktree's own directory in the main repository a worktree's `.git` file names,
/// `<main>/.git/worktrees/<name>`, by [`worktree_claim`]'s words alone. Its `gitdir` file is
/// where git records the worktree back.
pub fn worktree_entry(root: &Path, git_file: &[u8]) -> Option<PathBuf> {
    let named = git_file_target(git_file)?;
    let gitdir = normalized(&root.join(named))?;
    worktree_main(&gitdir)?;
    Some(gitdir)
}

/// The worktree's `.git` a main repository's `gitdir` file records, at `entry`: its first line,
/// taken relative to `entry` when it is relative, with `.` and `..` worked out by the words
/// alone. `None` for anything else.
pub fn worktree_backlink(entry: &Path, gitdir_file: &[u8]) -> Option<PathBuf> {
    let line = first_line(gitdir_file).filter(|line| !line.is_empty())?;
    normalized(&entry.join(line))
}

/// The repository a `.git` file names: its first line, `gitdir: <path>`. `None` for anything
/// else, or a path that is not UTF-8.
fn git_file_target(bytes: &[u8]) -> Option<PathBuf> {
    first_line(bytes)?
        .strip_prefix("gitdir: ")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
}

/// The first line of a short file git writes, without its line end. `None` for one longer
/// than [`MAX_GIT_FILE`], or one holding a NUL or what is not UTF-8.
fn first_line(bytes: &[u8]) -> Option<&str> {
    if bytes.len() as u64 > MAX_GIT_FILE || bytes.contains(&0) {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?;
    let line = text.split('\n').next().unwrap_or("");
    Some(line.strip_suffix('\r').unwrap_or(line))
}

/// Whether `a` and `b` name one place by their words: component by component, where a
/// verbatim prefix (`\\?\C:`, `\\?\UNC\host\share`) is the same as the plain one and a drive's
/// letter is the same in either case. `fs::canonicalize` gives verbatim roots on Windows, and
/// git writes plain paths. Nothing is looked up.
pub fn same_place(a: &Path, b: &Path) -> bool {
    let mut a = a.components();
    let mut b = b.components();
    loop {
        match (a.next(), b.next()) {
            (None, None) => return true,
            (Some(Component::Prefix(x)), Some(Component::Prefix(y))) => {
                if !same_prefix(x.kind(), y.kind()) {
                    return false;
                }
            }
            (x, y) if x == y => {}
            _ => return false,
        }
    }
}

/// Whether two prefixes name one drive or share, verbatim or not.
fn same_prefix(a: std::path::Prefix<'_>, b: std::path::Prefix<'_>) -> bool {
    use std::path::Prefix::{Disk, UNC, VerbatimDisk, VerbatimUNC};
    let plain = |prefix| match prefix {
        VerbatimDisk(letter) => Disk(letter.to_ascii_uppercase()),
        Disk(letter) => Disk(letter.to_ascii_uppercase()),
        VerbatimUNC(host, share) => UNC(host, share),
        other => other,
    };
    plain(a) == plain(b)
}

/// `path` with `.` dropped and each `..` taking the component before it away, by the words
/// alone. `None` if it is not absolute, or a `..` would climb past its root.
fn normalized(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                out.push(component);
            }
            Component::CurDir => {}
            Component::ParentDir => {
                // Climbing past the root names nothing.
                out.parent()?;
                out.pop();
            }
        }
    }
    Some(out)
}

/// The main repository's working directory, if `gitdir` has the shape git gives a worktree's
/// own directory: `<main>/.git/worktrees/<name>`.
fn worktree_main(gitdir: &Path) -> Option<&Path> {
    let mut up = gitdir.components().rev();
    let name = matches!(up.next()?, Component::Normal(_));
    let worktrees = up.next()? == Component::Normal("worktrees".as_ref());
    let dot_git = up.next()? == Component::Normal(".git".as_ref());
    if !(name && worktrees && dot_git) {
        return None;
    }
    gitdir.parent()?.parent()?.parent()
}

/// A project's root and, where the system has them, the directory's device and inode. A
/// directory deleted and made again at the same path is another one (ADR 0114).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RootIdentity {
    pub path: PathBuf,
    pub identity: Option<(u64, u64)>,
}

/// What the host found of a file's project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    /// The project's root.
    pub root: RootIdentity,
    /// The main repository the root's `.git` file names, if the root is a git worktree, by
    /// [`worktree_claim`]'s words alone. Not checked yet: it counts only once that repository
    /// is trusted and records this worktree back.
    pub worktree_of: Option<PathBuf>,
    /// The main repository [`Self::worktree_of`] names, once the host checked it: a trusted
    /// root, which records this worktree back. The project is trusted while this root is. Only
    /// the host's detector sets it, after that check; anything else leaves it `None`.
    pub checked_main: Option<RootIdentity>,
}

/// A project of the editor's table, which buffers point into.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ProjectId(pub(crate) u32);

/// A project the editor knows of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Project {
    found: Found,
    name: String,
}

impl Project {
    /// The project `found` is, named after its root's last component.
    pub(crate) fn new(found: Found) -> Self {
        let name = found.root.path.file_name().map_or_else(
            || found.root.path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        Self { found, name }
    }

    /// Its root.
    pub fn root(&self) -> &Path {
        &self.found.root.path
    }

    /// Its name, as the modeline shows it.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// What was found of it.
    pub(crate) fn found(&self) -> &Found {
        &self.found
    }

    /// What was found of it again: what its `.git` file names, and whether that was checked,
    /// may have changed.
    pub(crate) fn set_worktree(&mut self, found: Found) {
        self.found.worktree_of = found.worktree_of;
        self.found.checked_main = found.checked_main;
    }

    /// Whether `list` trusts it: its root, or the main repository it was checked to belong
    /// with.
    pub(crate) fn trusted_by(&self, list: &TrustList) -> bool {
        list.trusts(&self.found.root)
            || self
                .found
                .checked_main
                .as_ref()
                .is_some_and(|main| list.trusts(main))
    }
}

#[cfg(test)]
mod tests;
