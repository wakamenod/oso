//! Projects (ADR 0114): which directory a file's project is, from what the host found on
//! disk, and what the editor keeps of each.
//!
//! Must not depend on: `buffer`, `editor` or file I/O. These are pure functions of names
//! and bytes; the host looks at the disk, on a thread of its own, and runs nothing.
//!
//! A project is the nearest directory above a file that holds `.git`, `.jj` or `.hg`. A git
//! worktree's `.git` is a file naming `<main>/.git/worktrees/<name>`; when that directory's
//! own `gitdir` file names the worktree back, the worktree takes its main repository's trust.
//! The link back lives inside the main repository's `.git`, so no repository can claim
//! another's trust by naming it.

use std::path::{Component, Path, PathBuf};

/// The most bytes of a `.git` file or a `gitdir` file that are read. Git writes one short
/// line in each.
pub const MAX_GIT_FILE: u64 = 1024;

/// Which of the version-control markers a directory holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Markers {
    pub git: Option<GitMarker>,
    pub jj: bool,
    pub hg: bool,
}

impl Markers {
    /// Whether the directory holding these is a project's root.
    pub fn any(self) -> bool {
        self.git.is_some() || self.jj || self.hg
    }
}

/// What a directory's `.git` is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitMarker {
    /// A repository's own directory.
    Dir,
    /// A file naming the repository elsewhere, as a worktree's or a submodule's does.
    File,
}

/// The first of `dirs` whose markers make it a root, so the nearest when they go from the
/// file up, with its markers. `dirs` is lazy, so the host looks no further up than it must.
pub fn nearest_root<'a>(
    dirs: impl IntoIterator<Item = (&'a Path, Markers)>,
) -> Option<(&'a Path, Markers)> {
    dirs.into_iter().find(|(_, markers)| markers.any())
}

/// The repository a `.git` file names: its first line, `gitdir: <path>`. `None` for anything
/// else, or a path that is not UTF-8. A relative path is relative to the `.git` file's
/// directory, which the host joins.
pub fn git_file_target(bytes: &[u8]) -> Option<PathBuf> {
    first_line(bytes)?
        .strip_prefix("gitdir: ")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
}

/// The `.git` file a worktree's `gitdir` file names back: its first line.
pub fn worktree_backlink(bytes: &[u8]) -> Option<PathBuf> {
    first_line(bytes)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
}

/// The first line of `bytes`, without its end, if `bytes` is short, UTF-8 and holds no NUL.
fn first_line(bytes: &[u8]) -> Option<&str> {
    if bytes.len() as u64 > MAX_GIT_FILE || bytes.contains(&0) {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?;
    let line = text.split('\n').next().unwrap_or("");
    Some(line.strip_suffix('\r').unwrap_or(line))
}

/// The main repository's working directory, if `gitdir` is the shape git gives a worktree's
/// own directory: `<main>/.git/worktrees/<name>`. A submodule's (`.git/modules/…`) or a bare
/// repository's is not, and takes no trust from elsewhere.
pub fn worktree_main(gitdir: &Path) -> Option<&Path> {
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RootIdentity {
    pub path: PathBuf,
    pub identity: Option<(u64, u64)>,
}

/// What the host found of a file's project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    /// The project's root.
    pub root: RootIdentity,
    /// The main repository whose trust applies, for a worktree found to belong to it both
    /// ways. `None` when the root's own trust applies.
    pub trust_root: Option<RootIdentity>,
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
    pub fn found(&self) -> &Found {
        &self.found
    }

    /// What was found of it again: whether a worktree's link holds may have changed.
    pub(crate) fn set_trust_root(&mut self, trust_root: Option<RootIdentity>) {
        self.found.trust_root = trust_root;
    }
}

#[cfg(test)]
mod tests;
