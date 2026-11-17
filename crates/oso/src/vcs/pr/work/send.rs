//! A message for an agent session from a pull request's checks or review threads, on the
//! repository's `gh` thread (ADR 0127, step 6b).
//!
//! Must not depend on: `app`, `dispatch`, winit or the UI thread; this runs on `oso-vcs-gh`.
//!
//! - Everything is read again when it is asked for, as a pull request's reads are: the pull
//!   request, its checks or its threads, the login `gh` is logged in as, and the failed logs of
//!   at most [`MAX_LOGS`] GitHub Actions jobs of the repository itself, each within
//!   [`LOG_LIMIT`].
//! - The session it goes to is the one in the worktree its head branch is checked out in, as
//!   ADR 0122's step 12b finds it: git lists the worktree on that branch, and its commit holds
//!   the pull request's head. A fork's pull request has none.

use std::path::PathBuf;
use std::time::Duration;

use super::super::Bucket;
use super::super::issue::read_login;
use super::super::send::{
    Log, MAX_LOGS, Pick, PrMessage, SEND_FIELDS, SendPull, compose, job_of, log_tail, read_send,
};
use super::{Found, checks_of, run, threads_of, unexpected};
use crate::git::{GhAnswer, Git, PullTarget, list_worktrees};

/// How long one job's log may take to read.
const LOG_LIMIT: Duration = Duration::from_secs(10);
/// The most bytes of a job's failed log read: one past it is not read, and says so.
const MAX_LOG_ANSWER: usize = 4 * 1024 * 1024;
/// The most worktrees on the head branch looked at.
const MAX_CANDIDATES: usize = 8;
/// The most bytes of a message: ADR 0121's sends' bound.
pub(crate) const MAX_MESSAGE: usize = 256 << 10;

/// A message made for an agent session, and where it goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PrSent {
    pub(crate) number: u64,
    pub(crate) message: PrMessage,
    /// The worktree the pull request's head is checked out in, if one is.
    pub(crate) worktree: Option<PathBuf>,
}

pub(super) fn send(
    found: &Found,
    target: &PullTarget,
    number: u64,
    pick: &Pick,
) -> Result<PrSent, String> {
    let n = number.to_string();
    let repo = target.github.as_str();
    let answer = run(
        found,
        target,
        &["pr", "view", &n, "--repo", repo, "--json", SEND_FIELDS],
    )?;
    let pull = read_send(&answer.into_stdout()?).ok_or_else(unexpected)?;
    if pull.number != number {
        return Err(unexpected());
    }
    let (checks, threads) = if pick.of_threads() {
        (Vec::new(), threads_of(found, target, &n)?.0)
    } else {
        (checks_of(found, target, &n)?, Vec::new())
    };
    let picked_checks = pick.checks(&checks);
    let picked_threads = pick.threads(&threads);
    if picked_checks.is_empty() && picked_threads.is_empty() {
        return Err(nothing(pick, number));
    }
    let (host, ..) = target.github.parts();
    let person = run(found, target, &["api", "user", "--hostname", host])
        .and_then(GhAnswer::into_stdout)
        .ok()
        .and_then(|out| read_login(&out))
        .unwrap_or_default();
    let mut read = 0;
    let checks: Vec<_> = (picked_checks.into_iter())
        .map(|check| {
            let job = (check.bucket == Bucket::Fail)
                .then(|| job_of(&check.link, &target.github))
                .flatten();
            let log = match job {
                None => Log::None,
                Some(_) if read >= MAX_LOGS => Log::Unread(format!(
                    "only the first {MAX_LOGS} failing jobs' logs are read"
                )),
                Some((run_id, job)) => {
                    read += 1;
                    log_of(found, target, run_id, job)
                }
            };
            (check, log)
        })
        .collect();
    let message = compose(
        &target.github,
        &pull,
        &checks,
        &picked_threads,
        &person,
        MAX_MESSAGE,
    );
    let worktree = worktree_of(&found.git, &pull);
    Ok(PrSent {
        number,
        message,
        worktree,
    })
}

/// What is said when `pick` finds nothing in pull request `number`.
fn nothing(pick: &Pick, number: u64) -> String {
    match pick {
        Pick::Check { .. } => format!("#{number} has that check no more: g reads it again"),
        Pick::Failing => format!("No check of #{number} failed"),
        Pick::Thread { .. } => format!("#{number} has that thread no more: g reads it again"),
        Pick::Unresolved => format!("No review thread of #{number} is unresolved"),
    }
}

/// The tail of job `job` of run `run_id`'s failed steps' log.
fn log_of(found: &Found, target: &PullTarget, run_id: u64, job: u64) -> Log {
    if let Err(why) = found.root.check() {
        return Log::Unread(format!("not run: {why}"));
    }
    let (run_id, job) = (run_id.to_string(), job.to_string());
    let repo = target.github.as_str();
    let words = [
        "run",
        "view",
        &run_id,
        "--repo",
        repo,
        "--job",
        &job,
        "--log-failed",
    ];
    let answer = (found.gh).run(
        &target.github,
        &words,
        &found.cwd,
        MAX_LOG_ANSWER,
        LOG_LIMIT,
    );
    match answer.and_then(GhAnswer::into_stdout) {
        Ok(out) => Log::Tail(log_tail(&out)),
        Err(why) => Log::Unread(why),
    }
}

/// The worktree `pull`'s head branch is checked out in, if git lists one still there whose
/// commit holds the pull request's head; none for a fork's.
fn worktree_of(git: &Git, pull: &SendPull) -> Option<PathBuf> {
    if pull.cross {
        return None;
    }
    let listed = list_worktrees(git).ok()?;
    (listed.into_iter())
        .filter(|listed| !listed.bare && !listed.prunable)
        .filter(|listed| listed.branch.as_deref() == Some(pull.branch.as_str()))
        .take(MAX_CANDIDATES)
        .find(|listed| {
            (listed.head.as_deref())
                .is_some_and(|tip| git.is_ancestor(&pull.head_commit, tip).unwrap_or(false))
        })
        .map(|listed| listed.path)
}
