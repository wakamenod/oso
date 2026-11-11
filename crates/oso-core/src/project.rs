//! Projects (ADR 0114): what the host found of a file's project, read as plain data, and what
//! the editor keeps of each.
//!
//! Must not depend on: `buffer`, `editor` or file I/O. These are pure functions of names and
//! bytes; the host looks at the disk, on a thread of its own, and runs nothing.
//!
//! A git worktree's `.git` is a file naming `<main>/.git/worktrees/<name>`. What it names is
//! kept here by its words alone, never followed: it counts for trust only once the main
//! repository it names is trusted and records this worktree back (ADR 0114, step 2).

use std::path::{Component, Path, PathBuf};

/// The most bytes of a `.git` file that are read. Git writes one short line in it.
pub const MAX_GIT_FILE: u64 = 1024;

/// The main repository a worktree's `.git` file names, if it names one: its first line is
/// `gitdir: <path>`, and the path, taken relative to `root` when it is relative and with `.`
/// and `..` worked out by its words, has the shape `<main>/.git/worktrees/<name>`. Nothing
/// is looked up on disk. `None` for anything else, a submodule's `.git/modules/…` among them.
pub fn worktree_claim(root: &Path, git_file: &[u8]) -> Option<PathBuf> {
    let named = git_file_target(git_file)?;
    let gitdir = normalized(&root.join(named))?;
    worktree_main(&gitdir).map(Path::to_path_buf)
}

/// The repository a `.git` file names: its first line, `gitdir: <path>`. `None` for anything
/// else, or a path that is not UTF-8.
fn git_file_target(bytes: &[u8]) -> Option<PathBuf> {
    if bytes.len() as u64 > MAX_GIT_FILE || bytes.contains(&0) {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?;
    let line = text.split('\n').next().unwrap_or("");
    let line = line.strip_suffix('\r').unwrap_or(line);
    line.strip_prefix("gitdir: ")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
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
    /// The main repository the root's `.git` file names, if the root is a git worktree, by
    /// [`worktree_claim`]'s words alone. Not checked yet: it counts only once that repository
    /// is trusted and records this worktree back.
    pub worktree_of: Option<PathBuf>,
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

    /// What was found of it again: what its `.git` file names may have changed.
    pub(crate) fn set_worktree_of(&mut self, worktree_of: Option<PathBuf>) {
        self.found.worktree_of = worktree_of;
    }
}

#[cfg(test)]
mod tests;
