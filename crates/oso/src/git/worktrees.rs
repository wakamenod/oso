//! The worktrees `git worktree list --porcelain -z` lists, parsed (ADR 0121, "Worktree
//! sessions"; ADR 0122, step 12b, for the worktree a pull request's head is checked out in).
//! Git's output is untrusted: a path or a branch name may hold any byte, so nothing here
//! panics, and the list has a bound.
//!
//! Must not depend on: `app`, `dispatch`, winit or the UI thread, nor on what uses it; [`list`]
//! runs on the threads that run git, and waits for git; the parser is a plain function over
//! bytes.

use std::ffi::OsStr;
use std::path::PathBuf;

use super::path::os_lossy;
use super::{Git, GitError};

/// The most bytes of `git worktree list` read.
const MAX_LIST_BYTES: usize = 4 * 1024 * 1024;
/// The most worktrees a repository's listing keeps.
pub(crate) const MAX_WORKTREES: usize = 256;

/// The worktrees of the repository `git` runs in, the main one first, as git lists them, at
/// most [`MAX_WORKTREES`].
///
/// # Errors
///
/// If git cannot list them, or lists more than [`MAX_LIST_BYTES`].
pub(crate) fn list(git: &Git) -> Result<Vec<ListedWorktree>, GitError> {
    let args = ["worktree", "list", "--porcelain", "-z"].map(OsStr::new);
    let out = git.run(&args, &[], MAX_LIST_BYTES)?;
    Ok(worktrees(&out, MAX_WORKTREES))
}

/// One worktree as `git worktree list --porcelain -z` gives it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ListedWorktree {
    /// Where it is.
    pub(crate) path: PathBuf,
    /// The commit checked out, when git named one in UTF-8.
    pub(crate) head: Option<String>,
    /// The branch's short name, without `refs/heads/`, when it is UTF-8.
    pub(crate) branch: Option<String>,
    /// The main worktree of a bare repository.
    pub(crate) bare: bool,
    /// No branch is checked out.
    pub(crate) detached: bool,
    /// `git worktree lock` holds it.
    pub(crate) locked: bool,
    /// Git would prune it: its directory is gone.
    pub(crate) prunable: bool,
}

/// The worktrees of `git worktree list --porcelain -z`: fields each ended by NUL, a record ended
/// by an empty field. Fields are `worktree <path>`, `HEAD <oid>`, `branch refs/heads/<name>`,
/// `bare`, `detached`, `locked[ <reason>]` and `prunable[ <reason>]`; others are ignored. A
/// record without `worktree` is dropped. At most `max` records are kept.
pub(crate) fn worktrees(bytes: &[u8], max: usize) -> Vec<ListedWorktree> {
    let mut listed = Vec::new();
    let mut record: Option<ListedWorktree> = None;
    for field in ended(bytes) {
        if listed.len() >= max {
            break;
        }
        if field.is_empty() {
            listed.extend(record.take());
        } else if let Some(path) = field.strip_prefix(b"worktree ") {
            // A record git did not end goes in as it is.
            let next = ListedWorktree {
                path: os_lossy(path),
                ..ListedWorktree::default()
            };
            listed.extend(record.replace(next));
        } else if let Some(record) = record.as_mut() {
            read_field(record, field);
        }
    }
    listed.extend(record);
    listed.truncate(max);
    listed
}

/// The NUL-ended fields of `bytes`: what follows the last NUL is no field, as when output was
/// cut short.
fn ended(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    let end = bytes.iter().rposition(|&b| b == 0);
    end.map(|end| &bytes[..end])
        .into_iter()
        .flat_map(|fields| fields.split(|&b| b == 0))
}

/// Sets what `field` says on `record`.
fn read_field(record: &mut ListedWorktree, field: &[u8]) {
    let flag = |name: &[u8]| {
        field == name || (field.starts_with(name) && field.get(name.len()) == Some(&b' '))
    };
    if let Some(oid) = field.strip_prefix(b"HEAD ") {
        record.head = std::str::from_utf8(oid).ok().map(str::to_owned);
    } else if let Some(name) = field.strip_prefix(b"branch ") {
        let name = name.strip_prefix(b"refs/heads/").unwrap_or(name);
        record.branch = std::str::from_utf8(name)
            .ok()
            .filter(|name| !name.is_empty())
            .map(str::to_owned);
    } else if field == b"bare" {
        record.bare = true;
    } else if field == b"detached" {
        record.detached = true;
    } else if flag(b"locked") {
        record.locked = true;
    } else if flag(b"prunable") {
        record.prunable = true;
    }
}

#[cfg(test)]
mod tests;
