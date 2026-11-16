//! Branches on the repository's run thread (ADR 0127, step 3b): listing them for a prompt and
//! planning a delete, which are reads, and checking one out, creating, renaming and deleting.
//!
//! Must not depend on: `app`, `dispatch`, winit or the UI thread; these run on `oso-vcs-run`
//! and wait for git.
//!
//! - Reads use the plain git; the runs that change refs have the repository's own hooks
//!   (`post-checkout`, the reference transaction's), as a commit does.
//! - A forced delete is refused if the branch moved since its question, which named what it
//!   would drop then.

use std::ffi::OsStr;

use super::super::branch::{self, BranchRead, BranchRun, Branches, DeletePlan};
use super::super::remote::check_branch;
use super::remotes::{MAX_LISTED, current_branch, dropped, tip_of};
use super::stream::{ended_as, stream};
use super::{RunEnd, RunReport, Watch};
use crate::git::{Git, MAX_SHORT, for_git};

/// The most bytes of a list of branches read.
const MAX_LIST: usize = 1024 * 1024;

/// What `read` reports, with `found`, the plain git, or why there is none.
pub(super) fn read(found: Result<Git, String>, read: BranchRead) -> RunReport {
    match read {
        BranchRead::List => RunReport::Branches(found.and_then(|git| list(&git))),
        BranchRead::PlanDelete { name } => RunReport::DeletePlanned {
            plan: found.and_then(|git| plan_delete(&git, &name)),
            name,
        },
    }
}

/// Runs `run` with the names `pass` passes on to the hooks, to its end.
pub(super) fn run(
    git: &Git,
    run: &BranchRun,
    pass: &[String],
    watch: &Watch<'_>,
    send: &mut impl FnMut(RunReport) -> bool,
) -> RunEnd {
    let words = match words(git, run) {
        Ok(words) => words,
        Err(why) => return RunEnd::Refused(why),
    };
    let title = run.title();
    let streamed = match stream(&git.with_own_hooks(pass), &words, None, watch, send, false) {
        Ok(streamed) => streamed,
        Err(why) => return RunEnd::Refused(why),
    };
    ended_as(streamed, &title, |tail| branch::failed(&title, tail))
}

/// git's arguments for `run`, once its names pass and a forced delete's branch is where its
/// question saw it.
fn words(git: &Git, run: &BranchRun) -> Result<Vec<String>, String> {
    match run {
        BranchRun::Checkout { name, remote } => {
            check_branch(name)?;
            Ok(branch::checkout_args(name, *remote))
        }
        BranchRun::Create {
            name,
            start,
            checkout,
        } => {
            check_branch(name)?;
            branch::check_start(start)?;
            Ok(branch::create_args(name, start, *checkout))
        }
        BranchRun::Rename { old, new } => {
            check_branch(old)?;
            check_branch(new)?;
            Ok(branch::rename_args(old, new))
        }
        BranchRun::Delete { name, tip } => {
            check_branch(name)?;
            let now = tip_of(git, &format!("refs/heads/{name}"));
            if tip.is_some() && now != *tip {
                return Err(format!("{name} moved since the question: b k asks again"));
            }
            Ok(branch::delete_args(name, tip.is_some()))
        }
    }
}

/// The local and remote branches, at most [`MAX_LISTED`] each, and the one checked out. git
/// stops a little past what is kept, and a read cut at [`MAX_LIST`] keeps its whole lines.
fn list(git: &Git) -> Result<Branches, String> {
    let read = |refs: &str| -> Result<Vec<String>, String> {
        let count = format!("--count={}", MAX_LISTED + 64);
        let words = ["for-each-ref", &count, "--format=%(refname:lstrip=2)", refs];
        let words: Vec<&OsStr> = words.iter().map(OsStr::new).collect();
        let out = git
            .output(&words, &[], MAX_LIST)
            .map_err(|e| e.to_string())?;
        if !out.cut && !out.status.success() {
            return Err(out.stderr);
        }
        let whole = if out.cut {
            (out.stdout.iter())
                .rposition(|&byte| byte == b'\n')
                .map_or(0, |at| at + 1)
        } else {
            out.stdout.len()
        };
        Ok(branch::names(&out.stdout[..whole], MAX_LISTED))
    };
    Ok(Branches {
        local: read("refs/heads")?,
        remote: read("refs/remotes")?,
        current: current_branch(git).ok(),
    })
}

/// Whether `name` may be deleted with `-d`, or what a forced delete would drop; refused while
/// a worktree has it checked out.
fn plan_delete(git: &Git, name: &str) -> Result<DeletePlan, String> {
    check_branch(name)?;
    let words = ["worktree", "list", "--porcelain", "-z"].map(OsStr::new);
    let listed = git
        .run(&words, &[], branch::MAX_WORKTREE_LIST)
        .map_err(|e| e.to_string())?;
    if let Some(path) = branch::checked_out_in(&listed, name) {
        let here = (std::path::Path::new(&path).canonicalize().ok())
            .zip(git.top().canonicalize().ok())
            .is_some_and(|(a, b)| for_git(&a) == for_git(&b));
        return Err(branch::checked_out(name, &path, here));
    }
    let full = format!("refs/heads/{name}");
    let tip = tip_of(git, &full).ok_or_else(|| format!("No branch {name}"))?;
    let upstream = format!("{full}@{{upstream}}");
    let target = tip_of(git, &upstream).map_or_else(|| "HEAD".to_owned(), |_| upstream);
    let words = ["merge-base", "--is-ancestor", &tip, &target].map(OsStr::new);
    if git
        .output(&words, &[], MAX_SHORT)
        .is_ok_and(|out| out.status.success())
    {
        return Ok(DeletePlan::Merged);
    }
    // Before `--branches`, a pattern names a branch without `refs/heads/`.
    let exclude = format!("--exclude={name}");
    let only = [tip.as_str(), "--not", &exclude, "--branches", "--remotes"];
    Ok(DeletePlan::Unmerged {
        dropped: dropped(git, &only),
        tip,
    })
}
