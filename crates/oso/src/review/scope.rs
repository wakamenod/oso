//! What a review compares (ADR 0122): each scope's old and new side, the file list git gives
//! for them, and the session's baseline.
//!
//! Must not depend on: `app`, `dispatch` or winit; this runs git on the review's thread.
//!
//! | Scope | Old side | New side |
//! |---|---|---|
//! | Session | the baseline tree | the work tree, untracked files included |
//! | Uncommitted | `HEAD` (the empty tree on an unborn branch) | the work tree, untracked included |
//! | Staged | `HEAD` | the index |
//! | Unstaged | the index | the work tree's tracked files |
//! | Branch | `merge-base BASE HEAD` | the work tree, untracked included |
//! | Commit | its first parent (the empty tree for a root commit) | the commit |
//! | Range | `FROM`'s first parent | `TO` |
//!
//! Untracked files are listed through a copy of the index in which they are marked as to be
//! added (`git add -A -N`), which writes no file's contents into the repository. The baseline
//! is a tree written from such a copy with everything added; it is not referenced, so git
//! keeps it for its prune grace period, two weeks by default.

use std::ffi::OsStr;
use std::path::Path;

use super::git::{Git, GitError, MAX_SHORT, MAX_STDERR};
use super::id::{Id, trim_line};
use super::index::Scratch;
use super::list::{self, Listing};

/// The most bytes of `git diff --raw` output read.
pub(crate) const MAX_LIST_BYTES: usize = 32 * 1024 * 1024;

/// What to review. Revisions are as a person or an agent gave them, and are checked and
/// resolved to ids before git sees anything else.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Scope {
    Session { baseline: Id },
    Uncommitted,
    Staged,
    Unstaged,
    Branch { base: String },
    Commit { revision: String },
    Range { from: String, to: String },
}

/// Where a scope's new side comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum New {
    /// Files read from the work tree; `untracked` if files git does not track are listed too.
    WorkTree {
        untracked: bool,
    },
    Index,
    Tree(Id),
}

/// A scope resolved to ids.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Sides {
    /// The old side's commit or tree; `None` for the index.
    pub(crate) old: Option<Id>,
    pub(crate) new: New,
}

/// Why a scope could not be read.
#[derive(Debug)]
pub(crate) enum ScopeError {
    Git(GitError),
    /// A revision names no commit.
    NoSuchRevision(String),
    /// The session's baseline is gone, as git prunes an unreferenced tree.
    BaselineGone,
    /// A commit has a parent this clone does not hold, as a shallow clone's oldest have.
    NoParent,
    /// git found a work tree whose top is not the trusted root.
    Elsewhere,
    /// A scope that needs a copy of the index, and no private place to make one.
    NoScratch,
    /// The copy could not be made.
    Copy(std::io::Error),
}

impl From<GitError> for ScopeError {
    fn from(e: GitError) -> Self {
        Self::Git(e)
    }
}

/// The commit `revision` names, or why not.
fn commit(git: &Git, revision: &str) -> Result<Id, ScopeError> {
    git.resolve(revision, "commit")?
        .ok_or_else(|| ScopeError::NoSuchRevision(revision.to_owned()))
}

/// `HEAD`, or the empty tree on an unborn branch.
fn head_or_empty(git: &Git) -> Result<Id, ScopeError> {
    match git.resolve("HEAD", "commit")? {
        Some(head) => Ok(head),
        None => Ok(git.empty_tree()?),
    }
}

/// The first parent of `id`, or the empty tree for a root commit. A commit whose parent this
/// clone lacks is refused, rather than read as if it added every file.
fn parent_or_empty(git: &Git, id: &Id) -> Result<Id, ScopeError> {
    if let Some(parent) = git.resolve(&format!("{}^1", id.as_str()), "commit")? {
        return Ok(parent);
    }
    // A commit names its parents in its header, before its message.
    let args = [OsStr::new("cat-file"), OsStr::new("commit"), id.as_os_str()];
    let output = git.output(&args, &[], MAX_SHORT)?;
    let mut header = output
        .stdout
        .split(|&byte| byte == b'\n')
        .take_while(|line| !line.is_empty());
    if header.any(|line| line.starts_with(b"parent ")) {
        return Err(ScopeError::NoParent);
    }
    Ok(git.empty_tree()?)
}

/// Resolves `scope` to the ids it compares.
pub(crate) fn resolve(git: &Git, scope: &Scope) -> Result<Sides, ScopeError> {
    let work_tree = New::WorkTree { untracked: true };
    Ok(match scope {
        Scope::Session { baseline } => Sides {
            old: Some(
                git.resolve(baseline.as_str(), "tree")?
                    .ok_or(ScopeError::BaselineGone)?,
            ),
            new: work_tree,
        },
        Scope::Uncommitted => Sides {
            old: Some(head_or_empty(git)?),
            new: work_tree,
        },
        Scope::Staged => Sides {
            old: Some(head_or_empty(git)?),
            new: New::Index,
        },
        Scope::Unstaged => Sides {
            old: None,
            new: New::WorkTree { untracked: false },
        },
        Scope::Branch { base } => {
            let base = commit(git, base)?;
            let head = commit(git, "HEAD")?;
            let args = [OsStr::new("merge-base"), base.as_os_str(), head.as_os_str()];
            let out = git.run(&args, &[], MAX_SHORT)?;
            let fork =
                Id::parse(trim_line(&out)).ok_or(GitError::Unexpected("not an object id"))?;
            Sides {
                old: Some(fork),
                new: work_tree,
            }
        }
        Scope::Commit { revision } => {
            let id = commit(git, revision)?;
            Sides {
                old: Some(parent_or_empty(git, &id)?),
                new: New::Tree(id),
            }
        }
        Scope::Range { from, to } => {
            let from = commit(git, from)?;
            let to = commit(git, to)?;
            Sides {
                old: Some(parent_or_empty(git, &from)?),
                new: New::Tree(to),
            }
        }
    })
}

/// The files `sides` differ in. `index` is the repository's index, copied into `scratch` when
/// untracked files are listed.
pub(crate) fn list(
    git: &Git,
    sides: &Sides,
    index: &Path,
    scratch: Option<&mut Scratch>,
) -> Result<Listing, ScopeError> {
    let mut args: Vec<&OsStr> = [
        "diff",
        "--raw",
        "-z",
        "-M",
        "--no-abbrev",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
    ]
    .map(OsStr::new)
    .to_vec();
    if sides.new == New::Index {
        args.push(OsStr::new("--cached"));
    }
    args.extend(sides.old.as_ref().map(Id::as_os_str));
    if let New::Tree(new) = &sides.new {
        args.push(new.as_os_str());
    }
    args.push(OsStr::new("--"));
    let copy = match sides.new {
        New::WorkTree { untracked: true } => {
            let scratch = scratch.ok_or(ScopeError::NoScratch)?;
            let copy = scratch.copy(index).map_err(ScopeError::Copy)?;
            let set = [("GIT_INDEX_FILE", copy.path().as_os_str())];
            git.run(&["add", "-A", "-N", "--"].map(OsStr::new), &set, MAX_STDERR)?;
            Some(copy)
        }
        _ => None,
    };
    let set: Vec<(&str, &OsStr)> = copy
        .iter()
        .map(|copy| ("GIT_INDEX_FILE", copy.path().as_os_str()))
        .collect();
    let output = git.output(&args, &set, MAX_LIST_BYTES)?;
    if !output.cut && !output.status.success() {
        return Err(GitError::Failed {
            status: output.status.code(),
            stderr: output.stderr,
        }
        .into());
    }
    Ok(list::parse(&output.stdout, output.cut))
}

/// Takes a baseline: the work tree with its untracked files, as a tree, through a copy of
/// `index`. It gives the tree and at least how many files git could not read and left out:
/// the count is of the lines git wrote within [`MAX_STDERR`] of its own words, in the C locale.
pub(crate) fn baseline(
    git: &Git,
    index: &Path,
    scratch: &mut Scratch,
) -> Result<(Id, usize), ScopeError> {
    let copy = scratch.copy(index).map_err(ScopeError::Copy)?;
    let set = [
        ("GIT_INDEX_FILE", copy.path().as_os_str()),
        // The count below reads git's own words.
        ("LC_ALL", OsStr::new("C")),
    ];
    let added = git.output(
        &["add", "-A", "--ignore-errors", "--"].map(OsStr::new),
        &set,
        MAX_STDERR,
    )?;
    let left_out = added
        .stderr
        .lines()
        .filter(|line| line.starts_with("error: unable to index file"))
        .count();
    if added.cut {
        return Err(GitError::TooLong.into());
    }
    if !added.status.success() && left_out == 0 {
        return Err(GitError::Failed {
            status: added.status.code(),
            stderr: added.stderr,
        }
        .into());
    }
    let out = git.run(&[OsStr::new("write-tree")], &set, MAX_SHORT)?;
    let tree = Id::parse(trim_line(&out)).ok_or(GitError::Unexpected("not a tree id"))?;
    Ok((tree, left_out))
}
