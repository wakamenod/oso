//! Branches from a git status (ADR 0127, step 3b): what checking out, creating, renaming and
//! deleting ask for, run and say, what the prompts list, which worktree has a branch checked
//! out, and the question before deleting one that is not merged.
//!
//! Must not depend on: `app`, `dispatch`, git or the file system; these are pure functions of
//! names and of a child's output, which is untrusted: a record that does not parse is skipped,
//! and nothing here panics or grows past its input.
//!
//! - A name typed or picked is refused before git sees it when it could read as anything else
//!   on a command line ([`super::remote::check_branch`]); git's `check-ref-format` still applies.
//! - Checking out a remote's branch makes a local one that tracks it (`git switch --track`);
//!   a local one is switched to with `--no-guess`, so that git makes nothing of its own.
//! - A branch checked out in a worktree is not deleted: git refuses it, and oso says so first,
//!   pointing at `worktree-remove` (Q8).
//! - Deleting runs `git branch -d`; one that is not merged asks first, naming the commits only
//!   it holds, at most [`super::remote::MAX_DROPPED`], before `git branch -D`.

use oso_core::editor::row_text;

use super::remote::check_branch;

/// The most bytes of `git worktree list --porcelain -z` read.
pub(crate) const MAX_WORKTREE_LIST: usize = 1024 * 1024;

/// What the run thread reads for a branch's head, with the plain git: no hook runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum BranchRead {
    /// The branches for a prompt.
    List,
    /// Whether `name` may be deleted as it is, or asks first.
    PlanDelete { name: String },
}

/// What a branch's run does, with the repository's own hooks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum BranchRun {
    /// Check out `name`, a local branch, or a remote's if `remote`, as the prompt listed it.
    Checkout { name: String, remote: bool },
    /// Make `name` at `start`, checking it out if `checkout`.
    Create {
        name: String,
        start: String,
        checkout: bool,
    },
    /// Rename `old` to `new`.
    Rename { old: String, new: String },
    /// Delete `name`, with `-D` while it is still on `tip` if that is given.
    Delete { name: String, tip: Option<String> },
}

impl BranchRun {
    /// What the log and the echo area call it.
    pub(crate) fn title(&self) -> String {
        match self {
            Self::Checkout { name, .. } => format!("git switch {name}"),
            Self::Create { name, .. } => format!("git branch {name}"),
            Self::Rename { old, .. } => format!("git branch --move {old}"),
            Self::Delete { name, .. } => format!("git branch --delete {name}"),
        }
    }

    /// What the echo area says once it went well.
    pub(crate) fn done(&self) -> String {
        match self {
            Self::Checkout { name, .. } => format!("Checked out {name}"),
            Self::Create { name, start, .. } => {
                let start = (start.strip_prefix("refs/heads/"))
                    .or_else(|| start.strip_prefix("refs/remotes/"))
                    .unwrap_or(start);
                format!("Made {name} at {start}")
            }
            Self::Rename { old, new } => format!("Renamed {old} to {new}"),
            Self::Delete { name, .. } => format!("Deleted {name}"),
        }
    }
}

/// The branches a prompt lists: the local ones, the remote ones (`REMOTE/BRANCH`, not
/// `HEAD`), and the one checked out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Branches {
    pub(crate) local: Vec<String>,
    pub(crate) remote: Vec<String>,
    pub(crate) current: Option<String>,
}

/// What deleting a branch comes to, planned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DeletePlan {
    /// It is merged into its upstream, or `HEAD` without one: `-d` deletes it.
    Merged,
    /// It is not: `-D` after a question, with `tip`, its commit as asked, which it must still
    /// be on; the commits only it holds, at most five shown, escaped, and how many; `None` if
    /// git could not say, which the question says.
    Unmerged {
        tip: String,
        dropped: Option<(Vec<String>, usize)>,
    },
}

/// The names in `for-each-ref --format=%(refname:lstrip=2)`'s output that pass
/// [`check_branch`], but `REMOTE/HEAD`, at most `most`.
pub(crate) fn names(out: &[u8], most: usize) -> Vec<String> {
    (String::from_utf8_lossy(out).lines())
        .map(str::trim)
        .filter(|name| !name.is_empty() && !name.ends_with("/HEAD"))
        .filter(|name| check_branch(name).is_ok())
        .take(most)
        .map(str::to_owned)
        .collect()
}

/// What checking out `name` runs: a remote's if `remote`, which a local branch tracking it is
/// made for, else a local branch.
pub(crate) fn checkout_args(name: &str, remote: bool) -> Vec<String> {
    if remote {
        // In full, so that no tag or local branch of the same short name stands in for it.
        let full = format!("refs/remotes/{name}");
        vec!["switch".into(), "--track".into(), full]
    } else {
        vec!["switch".into(), "--no-guess".into(), name.into()]
    }
}

/// What making `name` at `start` runs, checking it out if `checkout`.
pub(crate) fn create_args(name: &str, start: &str, checkout: bool) -> Vec<String> {
    if checkout {
        vec![
            "switch".into(),
            "--create".into(),
            name.into(),
            start.into(),
        ]
    } else {
        vec!["branch".into(), name.into(), start.into()]
    }
}

/// What renaming `old` to `new` runs.
pub(crate) fn rename_args(old: &str, new: &str) -> Vec<String> {
    vec!["branch".into(), "--move".into(), old.into(), new.into()]
}

/// What deleting `name` runs, with `-D` if `force`.
pub(crate) fn delete_args(name: &str, force: bool) -> Vec<String> {
    let flag = if force { "-D" } else { "-d" };
    vec!["branch".into(), flag.into(), name.into()]
}

/// Refuses a start that git could read as an option or that holds a control character.
pub(crate) fn check_start(start: &str) -> Result<(), String> {
    crate::git::check_revision(start)
        .map_err(|_| "That is not a revision oso hands git: name a branch, a tag or an id".into())
}

/// The worktree that has `branch` checked out, from `git worktree list --porcelain -z`'s
/// output: its path, lossily.
pub(crate) fn checked_out_in(out: &[u8], branch: &str) -> Option<String> {
    let want = format!("branch refs/heads/{branch}");
    let mut path = None;
    for field in out.split(|&byte| byte == 0) {
        if field.is_empty() {
            path = None;
        } else if let Some(at) = field.strip_prefix(b"worktree ") {
            path = Some(String::from_utf8_lossy(at).into_owned());
        } else if field == want.as_bytes() {
            return path.or_else(|| Some(String::new()));
        }
    }
    None
}

/// Why `branch` is not deleted, checked out in the worktree at `path`, which is this one if it
/// is `here`.
pub(crate) fn checked_out(branch: &str, path: &str, here: bool) -> String {
    if here {
        format!("{branch} is checked out here: check out another branch first")
    } else {
        format!(
            "{branch} is checked out in the worktree at {}: worktree-remove removes it",
            row_text(path)
        )
    }
}

/// The question before deleting `branch`, which is not merged, naming what only it holds,
/// already escaped, or that oso could not tell.
pub(crate) fn delete_question(branch: &str, dropped: Option<&(Vec<String>, usize)>) -> String {
    let Some((named, count)) = dropped else {
        return format!(
            "Delete {branch}? It is not merged into its upstream or HEAD, and oso could not tell \
             which commits only it holds"
        );
    };
    let count = *count;
    if count == 0 {
        return format!(
            "Delete {branch}? It is not merged into its upstream or HEAD, but each of its \
             commits is on another branch or a remote"
        );
    }
    let commits = if count == 1 { "commit" } else { "commits" };
    let more = if count > named.len() { ", …" } else { "" };
    format!(
        "Delete {branch}? It holds {count} {commits} no other branch or remote has: {}{more}",
        named.join(", ")
    )
}

/// What the echo area says once `title` failed with `output`: a branch checked out in a
/// worktree points at `worktree-remove`.
pub(crate) fn failed(title: &str, output: &str) -> String {
    let lines = || output.lines().map(str::trim_start);
    if lines().any(|line| {
        line.contains("is already used by worktree at") || line.contains("checked out at")
    }) {
        return format!(
            "{title}: that branch is checked out in another worktree; worktree-remove removes it"
        );
    }
    super::remote::failed(title, output)
}

#[cfg(test)]
mod tests;
