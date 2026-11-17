//! `git` and `gh` for worktree sessions (ADR 0121): a repository's main checkout, its linked
//! worktrees, and the pull requests `gh` knows of, each run bounded as ADR 0122's runner bounds
//! it, but a removal's `git worktree remove`.
//!
//! Must not depend on: `app`, `dispatch` or the UI thread; this runs on a worktree
//! job's thread and waits for git.
//!
//! - Every git run is [`git`](crate::git)'s runner's: oso's own arguments and environment,
//!   no hook and no file system monitor of the repository's (the author's choice, ADR 0121
//!   step 4), `GIT_TERMINAL_PROMPT=0`, output bounded, killed after 10 s; a removal's
//!   `git worktree remove` alone runs with no time limit (`checks::remove`). Nothing here
//!   fetches.
//! - A linked worktree is run in only once its claim and the main repository's record of it
//!   are checked, and then with `GIT_DIR` and `GIT_WORK_TREE` given, so git looks for no
//!   repository of its own.
//! - `gh` is [`git::Gh`](crate::git::Gh)'s, run in the repository's empty private directory,
//!   asked about the merged pull requests of a branch.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::porcelain::{self, Listed};
use crate::git::{Gh, GhRepo, Git, GitError, MAX_GH, MAX_SHORT, RUN_LIMIT};
use crate::project::linked_worktree;

/// The most bytes of a listing read: worktrees, branches, refs, paths.
pub const MAX_LISTING: usize = 4 * 1024 * 1024;
/// The most worktrees a repository's listing keeps.
pub const MAX_WORKTREES: usize = 256;
/// The most names a listing of branches or refs keeps.
pub const MAX_NAMES: usize = 10_000;
/// How many merged pull requests of a branch `gh` is asked for.
const GH_LIMIT: &str = "30";

/// A repository's main checkout, run in by git.
#[derive(Clone, Debug)]
pub struct Repo {
    git: Git,
    main: PathBuf,
    /// The empty private directory git takes for hooks, where `gh` runs.
    hooks: PathBuf,
}

/// Text for the echo area from a failed git run.
pub fn said(error: &GitError) -> String {
    crate::git::printable(&error.to_string(), 400)
}

impl Repo {
    /// git in the main checkout at `main`, a trusted project's root, taking `hooks`, an empty
    /// private directory, for its hooks.
    ///
    /// # Errors
    ///
    /// If git cannot be found, or kept from looking above `main`.
    pub fn open(main: &Path, hooks: &Path) -> Result<Repo, String> {
        let git = Git::new(main, hooks).map_err(|e| said(&e))?;
        Ok(Repo {
            git,
            main: main.to_owned(),
            hooks: hooks.to_owned(),
        })
    }

    /// The same repository, whose bounded git runs are killed after `limit`.
    #[cfg(all(test, unix))]
    pub fn with_limit(mut self, limit: std::time::Duration) -> Repo {
        self.git = self.git.with_limit(limit);
        self
    }

    /// The main checkout's root.
    pub fn main(&self) -> &Path {
        &self.main
    }

    /// git in the main checkout.
    pub fn git(&self) -> &Git {
        &self.git
    }

    /// The empty private directory.
    pub fn hooks(&self) -> &Path {
        &self.hooks
    }

    /// Runs git in the main checkout with `args`, keeping at most `limit` bytes.
    ///
    /// # Errors
    ///
    /// As [`Git::run`].
    pub fn run(&self, args: &[&str], limit: usize) -> Result<Vec<u8>, GitError> {
        run(&self.git, args, limit)
    }

    /// The repository's worktrees, the main one first, as git lists them.
    ///
    /// # Errors
    ///
    /// If git cannot list them.
    pub fn worktrees(&self) -> Result<Vec<Listed>, String> {
        let out = self
            .run(&["worktree", "list", "--porcelain", "-z"], MAX_LISTING)
            .map_err(|e| said(&e))?;
        Ok(porcelain::worktrees(&out, MAX_WORKTREES))
    }

    /// git in the linked worktree at `path`, if it is one of this repository's whose claim
    /// holds: its `.git` names this repository, which records it back.
    pub fn in_worktree(&self, path: &Path) -> Option<Git> {
        let entry = linked_worktree(&self.main, path)?;
        Some(self.git.for_worktree(&entry, path))
    }

    /// The local branches' short names.
    ///
    /// # Errors
    ///
    /// If git cannot list them.
    pub fn branches(&self) -> Result<Vec<String>, String> {
        let args = [
            "for-each-ref",
            "--format=%(refname:short)%00",
            "refs/heads/",
        ];
        let out = self.run(&args, MAX_LISTING).map_err(|e| said(&e))?;
        Ok(porcelain::names(&out, MAX_NAMES))
    }

    /// The remotes' names, `origin` first.
    pub fn remotes(&self) -> Vec<String> {
        let Ok(out) = self.run(&["remote"], MAX_SHORT * 4) else {
            return Vec::new();
        };
        let mut remotes: Vec<String> = String::from_utf8_lossy(&out)
            .lines()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .collect();
        remotes.sort_by_key(|name| name != "origin");
        remotes
    }

    /// The address of `remote`, as its configuration gives it.
    pub fn remote_url(&self, remote: &str) -> Option<String> {
        let out = self
            .run(&["remote", "get-url", "--", remote], MAX_SHORT)
            .ok()?;
        let url = String::from_utf8(out).ok()?;
        Some(url.trim().to_owned())
    }
}

/// Runs `git` with `args`, keeping at most `limit` bytes of its output.
///
/// # Errors
///
/// As [`Git::run`].
pub fn run(git: &Git, args: &[&str], limit: usize) -> Result<Vec<u8>, GitError> {
    let args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
    git.run(&args, &[], limit)
}

/// Runs `git` with `args` and gives its exit status's code, as `merge-base --is-ancestor`
/// answers with it.
///
/// # Errors
///
/// If git cannot run or ran too long.
pub fn status(git: &Git, args: &[&str]) -> Result<Option<i32>, GitError> {
    let args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
    git.output(&args, &[], MAX_SHORT)
        .map(|out| out.status.code())
}

/// A merged pull request of a branch, as `gh` gives it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergedPr {
    pub number: u64,
    /// Its head commit when it was merged.
    pub head: String,
    /// The branch it was merged into.
    pub base: String,
}

/// The merged pull requests whose head is `branch` in `repo`, `gh` run in `cwd`, an empty
/// private directory.
///
/// # Errors
///
/// Why `gh` gave no answer: it could not run, failed, ran past 10 s, or said something that is
/// not the JSON asked for.
pub fn merged_pulls(
    gh: &Gh,
    repo: &GhRepo,
    branch: &str,
    cwd: &Path,
) -> Result<Vec<MergedPr>, String> {
    let args = [
        "pr",
        "list",
        "--repo",
        repo.as_str(),
        "--head",
        branch,
        "--state",
        "merged",
        "--json",
        "number,headRefOid,baseRefName",
        "--limit",
        GH_LIMIT,
    ];
    let out = gh.run(repo, &args, cwd, MAX_GH, RUN_LIMIT)?.into_stdout()?;
    parse_merged(&out).ok_or_else(|| "gh gave an unexpected answer".to_owned())
}

/// The most bytes of a repository's open pull requests read.
const MAX_PULLS: usize = 4 * 1024 * 1024;

/// The open pull requests of `repo`, each its head branch and the line a worktree's row shows of
/// it (ADR 0127, step 5b-2), `gh` run in `cwd`, an empty private directory.
///
/// # Errors
///
/// Why `gh` gave no answer.
pub fn open_pulls(gh: &Gh, repo: &GhRepo, cwd: &Path) -> Result<Vec<(String, String)>, String> {
    let fields = crate::vcs::pr::LIST_FIELDS;
    let args = [
        "pr",
        "list",
        "--repo",
        repo.as_str(),
        "--state",
        "open",
        "--limit",
        "100",
        "--json",
        fields,
    ];
    let out = gh
        .run(repo, &args, cwd, MAX_PULLS, RUN_LIMIT)?
        .into_stdout()?;
    let items = crate::vcs::pr::read_list(&out).ok_or("gh gave an unexpected answer")?;
    Ok(items
        .iter()
        .map(|item| {
            (
                item.branch.clone(),
                crate::vcs::pr::write::pull_summary(item),
            )
        })
        .collect())
}

/// The repository on GitHub, or the host `gh_host` names, of `repo`'s first remote that names
/// one, `origin` first.
pub fn github_of(repo: &Repo, gh_host: Option<&str>) -> Option<GhRepo> {
    (repo.remotes().iter()).find_map(|remote| GhRepo::from_url(&repo.remote_url(remote)?, gh_host))
}

/// The pull requests in `gh pr list --json number,headRefOid,baseRefName`'s answer; `None` if it
/// is not that shape. Entries missing a field are left out.
pub fn parse_merged(json: &[u8]) -> Option<Vec<MergedPr>> {
    let Value::Array(items) = serde_json::from_slice::<Value>(json).ok()? else {
        return None;
    };
    Some(
        items
            .iter()
            .filter_map(|item| {
                Some(MergedPr {
                    number: item.get("number")?.as_u64()?,
                    head: item.get("headRefOid")?.as_str()?.to_owned(),
                    base: item.get("baseRefName")?.as_str()?.to_owned(),
                })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::{MergedPr, parse_merged};

    #[test]
    fn gh_answers_are_read_by_shape() {
        let json = br#"[{"number":12,"headRefOid":"abc","baseRefName":"main"},{"number":"x"}]"#;
        assert_eq!(
            parse_merged(json),
            Some(vec![MergedPr {
                number: 12,
                head: "abc".into(),
                base: "main".into()
            }])
        );
        assert_eq!(parse_merged(b"[]"), Some(Vec::new()));
        assert_eq!(parse_merged(b"{}"), None);
        assert_eq!(parse_merged(b"not json"), None);
    }
}
