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
//!
//! No git run enters a repository embedded in the work tree, which is a project of its own with
//! its own configuration: diffs take `--ignore-submodules=dirty`, which no `.gitmodules`
//! overrides, and the adds leave out every submodule the index copy names, by pathspecs on
//! their standard input. An embedded repository the index does not name yet is added as a
//! submodule, and nothing runs inside it.

use std::ffi::OsStr;
use std::io::{self, BufRead, BufReader, Read};
use std::path::Path;

use super::git::{Git, GitError, MAX_SHORT, MAX_STDERR, Output, for_git, read_at_most};
use super::id::{Id, trim_line};
use super::index::Scratch;
use super::list::{self, Listing};

/// The most bytes of `git diff --raw` output read.
pub(crate) const MAX_LIST_BYTES: usize = 32 * 1024 * 1024;
/// The most bytes of submodule paths an index copy may name for the adds to leave out.
const MAX_SUBMODULE_BYTES: usize = 1024 * 1024;
/// The longest record of `git ls-files -s -z` read: its mode, id and stage, and a path.
const MAX_STAGE_RECORD: u64 = 8 * 1024;

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
        "--ignore-submodules=dirty",
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
            let path = for_git(copy.path());
            let set = [("GIT_INDEX_FILE", path.as_os_str())];
            let added = add(git, &["add", "-A", "-N"], &set)?;
            if added.cut {
                return Err(GitError::TooLong.into());
            }
            if !added.status.success() {
                return Err(GitError::Failed {
                    status: added.status.code(),
                    stderr: added.stderr,
                }
                .into());
            }
            Some((copy, path))
        }
        _ => None,
    };
    let set: Vec<(&str, &OsStr)> = copy
        .iter()
        .map(|(_, path)| ("GIT_INDEX_FILE", path.as_os_str()))
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
    let path = for_git(copy.path());
    let set = [
        ("GIT_INDEX_FILE", path.as_os_str()),
        // The count below reads git's own words.
        ("LC_ALL", OsStr::new("C")),
    ];
    let added = add(git, &["add", "-A", "--ignore-errors"], &set)?;
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

/// Runs `git add` with `args` over the index copy `set` names, leaving out every submodule the
/// copy names, so that git starts nothing inside one.
fn add(git: &Git, args: &[&str], set: &[(&str, &OsStr)]) -> Result<Output, GitError> {
    let submodules = submodules(git, set)?;
    let mut args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
    if submodules.is_empty() {
        args.push(OsStr::new("--"));
        return git.output(&args, set, MAX_STDERR);
    }
    args.extend(["--pathspec-from-file=-", "--pathspec-file-nul"].map(OsStr::new));
    let mut specs = Vec::new();
    for path in &submodules {
        specs.extend_from_slice(b":(exclude,top,literal)");
        specs.extend_from_slice(path);
        specs.push(0);
    }
    git.output_with(&args, set, Some(specs), |stdout| {
        read_at_most(stdout, MAX_STDERR)
    })
}

/// The paths of the submodules the index copy `set` names, each once.
fn submodules(git: &Git, set: &[(&str, &OsStr)]) -> Result<Vec<Box<[u8]>>, GitError> {
    let args = ["ls-files", "-s", "-z"].map(OsStr::new);
    let output = git.output_with(&args, set, None, submodule_paths)?;
    if output.cut {
        return Err(GitError::TooLong);
    }
    if !output.status.success() {
        return Err(GitError::Failed {
            status: output.status.code(),
            stderr: output.stderr,
        });
    }
    let mut paths: Vec<Box<[u8]>> = output
        .stdout
        .split(|&byte| byte == 0)
        .filter(|path| !path.is_empty())
        .map(Box::from)
        .collect();
    // A conflicted submodule is named once per stage, one after another.
    paths.dedup();
    Ok(paths)
}

/// Reads `git ls-files -s -z` and keeps the paths of its submodules, each ended by a NUL, up to
/// [`MAX_SUBMODULE_BYTES`]; `true` if there were more, or a record past [`MAX_STAGE_RECORD`].
fn submodule_paths(stdout: impl Read) -> io::Result<(Vec<u8>, bool)> {
    let mut stdout = BufReader::new(stdout);
    let mut kept = Vec::new();
    let mut record = Vec::new();
    loop {
        record.clear();
        (&mut stdout)
            .take(MAX_STAGE_RECORD)
            .read_until(0, &mut record)?;
        match record.pop() {
            None => return Ok((kept, false)),
            Some(0) => {}
            Some(_) => return Ok((kept, true)),
        }
        // `MODE ID STAGE\tPATH`
        let Some(tab) = record.iter().position(|&byte| byte == b'\t') else {
            continue;
        };
        if !record.starts_with(b"160000 ") {
            continue;
        }
        kept.extend_from_slice(&record[tab + 1..]);
        kept.push(0);
        if kept.len() > MAX_SUBMODULE_BYTES {
            return Ok((kept, true));
        }
    }
}
