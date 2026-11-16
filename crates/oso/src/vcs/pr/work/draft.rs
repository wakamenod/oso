//! What making and merging a pull request read first, on the repository's `gh` thread (ADR
//! 0127, step 5a-2): the branch's commits its base lacks, the template and the base, whether the
//! branch is to be pushed first, and the open pull request a merge is of.
//!
//! Must not depend on: `app`, `dispatch`, winit or the UI thread; this runs on `oso-vcs-gh`.
//!
//! - The base is the one asked for, if the branch filter takes it, else the remote's `HEAD`,
//!   else the repository's default branch as `gh repo view` says (Q7).
//! - The template is the first of `.github/`, the root's and `docs/`'s, in any case, read only
//!   as a regular file, at most 64 KiB, and never one whose name may hold secrets (Q8).
//! - A branch with no upstream, or ahead of it, is to be pushed before its pull request is made.
//! - A merge is of the branch's pull request while it is open, with its head as `gh` says it.

use std::io::Read;
use std::path::Path;

use super::super::create::{
    BranchCommit, MAX_COMMITS, MAX_TEMPLATE, TEMPLATE_DIRS, TEMPLATE_NAME, draft_text, read_commits,
};
use super::super::{read_default_branch, read_merge};
use super::{Found, Target, no_pull_request, none_found, pr_view, read_line, run, unexpected};
use crate::git::may_hold_secrets;
use crate::vcs::read::{args, set};
use crate::vcs::remote::check_branch;

/// The most bytes of the branch's commits read.
const MAX_LOG: usize = 1024 * 1024;

/// What a pull request's message starts from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Drafted {
    pub(crate) text: String,
    pub(crate) base: String,
    pub(crate) branch: String,
    /// The repository, `HOST/OWNER/REPO`.
    pub(crate) repo: String,
    /// The remote the branch pushes to.
    pub(crate) remote: String,
    /// Whether the branch has no upstream, or is ahead of it.
    pub(crate) push: bool,
}

/// The open pull request a merge is of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MergeTarget {
    pub(crate) number: u64,
    pub(crate) base: String,
    /// Its head commit, whole.
    pub(crate) head: String,
    pub(crate) repo: String,
}

pub(super) fn draft(found: &Found, target: &Target, base: Option<&str>) -> Result<Drafted, String> {
    let branch =
        (target.branch.clone()).ok_or("No branch is checked out to make a pull request of")?;
    let remote = &target.remote;
    let base = match base {
        Some(base) => {
            check_branch(base).map_err(|_| "Not a branch name git takes".to_owned())?;
            base.to_owned()
        }
        None => default_base(found, target)?,
    };
    if base == branch {
        return Err(format!("{branch} is its own base: N c - b names another"));
    }
    let git = &found.git;
    let push = read_line(git, &["rev-parse", "--verify", "--quiet", "@{upstream}"]).is_none()
        || read_line(git, &["rev-list", "--count", "@{upstream}..HEAD"])
            .is_none_or(|ahead| ahead != "0");
    let commits = branch_commits(found, remote, &base);
    let template = template(found.root.path());
    Ok(Drafted {
        text: draft_text(&commits, template.as_deref()),
        base,
        branch,
        repo: target.github.as_str().to_owned(),
        remote: remote.clone(),
        push,
    })
}

/// The remote's `HEAD`, else the repository's default branch.
fn default_base(found: &Found, target: &Target) -> Result<String, String> {
    let remote = &target.remote;
    let head = format!("refs/remotes/{remote}/HEAD");
    let known = read_line(&found.git, &["symbolic-ref", "--quiet", "--short", &head])
        .and_then(|name| name.strip_prefix(&format!("{remote}/")).map(str::to_owned));
    if let Some(base) = known {
        return Ok(base);
    }
    let repo = target.github.as_str();
    let words = ["repo", "view", repo, "--json", "defaultBranchRef"];
    let answer = run(found, target, &words)?.into_stdout()?;
    let base = read_default_branch(&answer).ok_or_else(unexpected)?;
    check_branch(&base).map_err(|_| unexpected())?;
    Ok(base)
}

/// The branch's commits its base on `remote` lacks, newest first; just `HEAD` if the base is
/// not known there.
fn branch_commits(found: &Found, remote: &str, base: &str) -> Vec<BranchCommit> {
    let at = format!("refs/remotes/{remote}/{base}");
    let known = read_line(&found.git, &["rev-parse", "--verify", "--quiet", &at]).is_some();
    let range = if known {
        format!("{at}..HEAD")
    } else {
        "HEAD".to_owned()
    };
    let count = if known {
        MAX_COMMITS.to_string()
    } else {
        "1".to_owned()
    };
    let words = [
        "log",
        "-z",
        "--no-show-signature",
        "--no-color",
        "--format=%s%x1f%B",
        &format!("--max-count={count}"),
        &range,
        "--",
    ];
    (found.git.run(&args(&words), &set(), MAX_LOG))
        .map(|out| read_commits(&out))
        .unwrap_or_default()
}

/// The first template of the repository at `root` there is.
fn template(root: &Path) -> Option<String> {
    TEMPLATE_DIRS.iter().find_map(|dir_name| {
        let dir = root.join(dir_name);
        let entries = std::fs::read_dir(&dir).ok()?;
        let name = entries
            .filter_map(Result::ok)
            .map(|entry| entry.file_name())
            .find(|name| {
                name.to_str()
                    .is_some_and(|n| n.eq_ignore_ascii_case(TEMPLATE_NAME))
            })?;
        let relative = Path::new(dir_name).join(&name);
        if may_hold_secrets(root, relative.as_os_str().as_encoded_bytes()) {
            return None;
        }
        let path = dir.join(name);
        let (file, _) = oso_platform::file_meta::open_regular_here(&path).ok()??;
        let mut bytes = Vec::new();
        file.take(MAX_TEMPLATE as u64)
            .read_to_end(&mut bytes)
            .ok()?;
        Some(String::from_utf8_lossy(&bytes).into_owned())
    })
}

pub(super) fn merge_target(found: &Found, target: &Target) -> Result<MergeTarget, String> {
    let fields = "number,state,baseRefName,headRefOid,headRefName";
    let answer = pr_view(found, target, None, fields)?;
    if none_found(&answer) {
        return Err(no_pull_request(target));
    }
    let merge = read_merge(&answer.into_stdout()?).ok_or_else(unexpected)?;
    if Some(&merge.branch) != target.branch.as_ref() {
        return Err(no_pull_request(target));
    }
    if !merge.open {
        return Err(format!(
            "#{} is not open, so it is not merged",
            merge.number
        ));
    }
    Ok(MergeTarget {
        number: merge.number,
        base: merge.base,
        head: merge.head,
        repo: target.github.as_str().to_owned(),
    })
}
