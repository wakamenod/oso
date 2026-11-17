//! Issues on the repository's `gh` thread (ADR 0127, step 5b-1): the open ones, one with its
//! comments, and what a worktree session from one starts from.
//!
//! Must not depend on: `app`, `dispatch`, winit or the UI thread; this runs on `oso-vcs-gh`.
//!
//! - As a pull request's reads: `gh` outside the repository, about the repository the branch's
//!   remote names, the root looked at before each run.
//! - A worktree session from an issue gets the branch `issue-N-SLUG`, from the remote's default
//!   branch, and the brief `brief_text` makes, with the comments of the login `gh` is logged in
//!   as kept; a `gh` that cannot say who it is keeps none of them but the others' rules.

use serde_json::Value;

use super::super::issue::{
    LIST_FIELDS, VIEW_FIELDS, branch_name, brief_text, read_issue, read_issue_list,
};
use super::super::write::{PrLaid, issue_list_text, issue_text};
use super::create::default_base;
use super::{Found, Target, run, unexpected};
use crate::vcs::remote::check_branch;

/// What a worktree session from an issue starts from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct IssueBrief {
    pub(crate) number: u64,
    pub(crate) branch: String,
    pub(crate) base: String,
    pub(crate) text: String,
}

pub(super) fn list(found: &Found, target: &Target) -> Result<PrLaid, String> {
    let repo = target.github.as_str();
    let words = [
        "issue",
        "list",
        "--repo",
        repo,
        "--state",
        "open",
        "--limit",
        "100",
        "--json",
        LIST_FIELDS,
    ];
    let out = run(found, target, &words)?.into_stdout()?;
    let items = read_issue_list(&out).ok_or_else(unexpected)?;
    Ok(issue_list_text(repo, &items))
}

pub(super) fn view(found: &Found, target: &Target, number: u64) -> Result<PrLaid, String> {
    let issue = issue(found, target, number)?;
    Ok(issue_text(&issue))
}

pub(super) fn brief(found: &Found, target: &Target, number: u64) -> Result<IssueBrief, String> {
    let issue = issue(found, target, number)?;
    let person = run(found, target, &["api", "user"])
        .and_then(crate::git::GhAnswer::into_stdout)
        .ok()
        .and_then(|out| serde_json::from_slice::<Value>(&out).ok())
        .and_then(|user| Some(user.get("login")?.as_str()?.to_owned()))
        .unwrap_or_default();
    let branch = branch_name(issue.number, &issue.title);
    check_branch(&branch).map_err(|_| "The issue gives no branch name git takes".to_owned())?;
    Ok(IssueBrief {
        number: issue.number,
        branch,
        base: default_base(found, target)?,
        text: brief_text(&issue, &person),
    })
}

fn issue(
    found: &Found,
    target: &Target,
    number: u64,
) -> Result<super::super::issue::Issue, String> {
    let n = number.to_string();
    let repo = target.github.as_str();
    let words = ["issue", "view", &n, "--repo", repo, "--json", VIEW_FIELDS];
    let out = run(found, target, &words)?.into_stdout()?;
    read_issue(&out).ok_or_else(unexpected)
}
