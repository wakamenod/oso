//! Whether a worktree's branch is merged, the fifth check before `worktree-remove` (ADR 0121):
//! by ancestry against the remotes' main branches as they were last fetched, else by a merged
//! pull request `gh` knows of, which covers squash merges. Nothing is fetched.
//!
//! Must not depend on: `app`, `dispatch`, `commands` or the UI thread; this runs on a worktree
//! job's thread.
//!
//! - **Ancestry:** the branch's reflog shows a commit made on it, and its tip is in
//!   `refs/remotes/R/main`, `master`, `develop`, or the branch `refs/remotes/R/HEAD` points to,
//!   for any remote R. A branch with no commit of its own is not merged (the author's choice).
//! - **A pull request:** `gh pr list --head BRANCH --state merged` for the first remote whose
//!   address names a GitHub repository, `origin` first: one merged into such a branch whose head
//!   commit is the tip, or a commit here that the tip leads to. With nothing fetched a squash's
//!   merge commit never holds the tip, so what was merged is told by its head.
//! - No remote, or no `gh` when ancestry is not enough: not merged, saying which.

use std::collections::HashSet;

use super::git::{Gh, GhRepo, MAX_LISTING, MAX_NAMES, Repo, run, said, status};
use super::porcelain;
use crate::review::git::MAX_SHORT;

/// The branch names a remote's main line goes by, besides the one its `HEAD` points to.
pub const MAIN_BRANCHES: [&str; 3] = ["main", "master", "develop"];

/// The most bytes of a branch's reflog read.
const MAX_REFLOG: usize = 1024 * 1024;

/// Whether a branch is merged, and how that was found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Merged {
    /// Its tip is in this remote branch, such as `origin/main`.
    Ancestor(String),
    /// This merged pull request merged its tip.
    PullRequest(u64),
    /// Not merged, or not known to be, and why.
    No(String),
}

impl Merged {
    pub fn is_merged(&self) -> bool {
        !matches!(self, Self::No(_))
    }

    /// How it was found, for a person.
    pub fn how(&self) -> String {
        match self {
            Self::Ancestor(remote) => format!("merged (in {remote})"),
            Self::PullRequest(number) => format!("merged (pull request #{number})"),
            Self::No(why) => why.clone(),
        }
    }
}

/// Whether `branch`, whose tip is the commit `tip`, is merged in `repo`, asking `gh` when it
/// is given and ancestry is not enough.
pub fn merged(repo: &Repo, gh: Option<&Gh>, branch: &str, tip: &str) -> Merged {
    let remotes = repo.remotes();
    if remotes.is_empty() {
        return Merged::No("not merged: the repository has no remote".to_owned());
    }
    let mains = match main_refs(repo) {
        Ok(mains) => mains,
        Err(why) => return Merged::No(format!("not known: {why}")),
    };
    if made_a_commit(repo, branch)
        && let Some(remote) = containing(repo, tip, &mains)
    {
        return Merged::Ancestor(remote);
    }
    let Some(gh) = gh else {
        return Merged::No("not merged, as far as the remotes show; gh is not found".to_owned());
    };
    by_pull_request(repo, gh, &remotes, &mains, branch, tip)
}

/// The remote branches that count as main lines: `refs/remotes/R/{main,master,develop}` and
/// what each `refs/remotes/R/HEAD` points to, by full name.
fn main_refs(repo: &Repo) -> Result<HashSet<String>, String> {
    let args = [
        "for-each-ref",
        "--format=%(refname)%00%(symref)%00",
        "refs/remotes/",
    ];
    let out = repo.run(&args, MAX_LISTING).map_err(|e| said(&e))?;
    let mut mains = HashSet::new();
    for (name, target) in porcelain::ref_pairs(&out, MAX_NAMES) {
        let Some((remote, branch)) = name
            .strip_prefix("refs/remotes/")
            .and_then(|rest| rest.split_once('/'))
        else {
            continue;
        };
        if branch == "HEAD" {
            mains.extend(target.filter(|t| t.starts_with(&format!("refs/remotes/{remote}/"))));
        } else if MAIN_BRANCHES.contains(&branch) {
            mains.insert(name.clone());
        }
    }
    Ok(mains)
}

/// Whether the branch's reflog shows a commit made on it.
fn made_a_commit(repo: &Repo, branch: &str) -> bool {
    let reference = format!("refs/heads/{branch}");
    let args = ["log", "-g", "--format=%gs", "-z", &reference, "--"];
    repo.run(&args, MAX_REFLOG)
        .is_ok_and(|reflog| porcelain::made_a_commit(&reflog))
}

/// The first main line, by its short name, that holds `tip`.
fn containing(repo: &Repo, tip: &str, mains: &HashSet<String>) -> Option<String> {
    let contains = format!("--contains={tip}");
    let args = [
        "for-each-ref",
        &contains,
        "--format=%(refname)%00",
        "refs/remotes/",
    ];
    let out = repo.run(&args, MAX_LISTING).ok()?;
    let mut found: Vec<String> = porcelain::names(&out, MAX_NAMES)
        .into_iter()
        .filter(|name| mains.contains(name))
        .collect();
    found.sort();
    found
        .into_iter()
        .next()
        .map(|name| name.trim_start_matches("refs/remotes/").to_owned())
}

/// Whether `gh` knows of a merged pull request that merged `tip`, for the first remote that
/// names a GitHub repository.
fn by_pull_request(
    repo: &Repo,
    gh: &Gh,
    remotes: &[String],
    mains: &HashSet<String>,
    branch: &str,
    tip: &str,
) -> Merged {
    let Some((remote, github)) = remotes.iter().find_map(|remote| {
        let url = repo.remote_url(remote)?;
        Some((remote.as_str(), GhRepo::from_url(&url)?))
    }) else {
        return Merged::No("not merged, as far as the remotes show".to_owned());
    };
    let bases: HashSet<String> = mains
        .iter()
        .filter_map(|name| name.strip_prefix(&format!("refs/remotes/{remote}/")))
        .map(str::to_owned)
        .chain(MAIN_BRANCHES.map(str::to_owned))
        .collect();
    let pulls = match gh.merged(&github, branch, repo.hooks()) {
        Ok(pulls) => pulls,
        Err(why) => return Merged::No(format!("not known: {why}")),
    };
    pulls
        .iter()
        .filter(|pull| bases.contains(&pull.base))
        .find(|pull| pull.head == tip || leads_to(repo, tip, &pull.head))
        .map_or_else(
            || Merged::No("not merged: no merged pull request holds its tip".to_owned()),
            |pull| Merged::PullRequest(pull.number),
        )
}

/// Whether `tip` is an ancestor of `head`, a commit this repository has.
fn leads_to(repo: &Repo, tip: &str, head: &str) -> bool {
    if !head.bytes().all(|b| b.is_ascii_hexdigit()) || head.len() < 40 {
        return false;
    }
    let args = ["merge-base", "--is-ancestor", tip, head];
    matches!(status(repo.git(), &args), Ok(Some(0)))
}

/// The commit a branch's tip is, by its full name.
pub fn tip(repo: &Repo, branch: &str) -> Option<String> {
    let reference = format!("refs/heads/{branch}^{{commit}}");
    let args = [
        "rev-parse",
        "--verify",
        "--quiet",
        "--end-of-options",
        &reference,
    ];
    let out = run(repo.git(), &args, MAX_SHORT).ok()?;
    let id = String::from_utf8(out).ok()?;
    let id = id.trim();
    id.bytes()
        .all(|b| b.is_ascii_hexdigit())
        .then(|| id.to_owned())
        .filter(|id| !id.is_empty())
}
