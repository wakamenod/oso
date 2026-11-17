//! A pull request's runs on the repository's run thread (ADR 0127, step 5a-2): `gh pr create`
//! with the message on its standard input, and `gh pr merge` bound to the head the person was
//! asked about.
//!
//! Must not depend on: `app`, `dispatch`, winit or the UI thread; these run on `oso-vcs-run`
//! and wait for `gh`.
//!
//! - `gh` runs as the pull requests' thread runs it: outside the repository, in its own empty
//!   directory, about the repository the branch's remote names, which must still be the one the
//!   run was asked for; the project left out of its `PATH`.
//! - It runs in a process group of its own with no time limit, as git's long runs do: the
//!   person stops it with `C-g`, and its output goes to the log as it comes, each value quoted.
//! - A merge is still of what its question named: just before it, the pull request must be
//!   open and into the base named, and `--match-head-commit` binds its head.

use std::path::Path;

use super::super::pr::create::{PrRun, failed, log_line};
use super::super::pr::read_merge;
use super::super::pr::work::{GH_LIMIT, branch_repo};
use super::stream::{ended_as, stream_long};
use super::{RunEnd, RunReport, Watch};
use crate::git::{Gh, GhRepo, Git, MAX_GH, TrustedRoot};

/// Runs `run` about the repository at `root`, `gh`'s directory under `state`, to its end.
pub(super) fn run(
    git: &Git,
    (root, state): (&TrustedRoot, Option<&Path>),
    run: &PrRun,
    watch: &Watch<'_>,
    send: &mut impl FnMut(RunReport) -> bool,
) -> RunEnd {
    let github = match branch_repo(git) {
        Ok(github) => github,
        Err(why) => return RunEnd::Refused(why),
    };
    if github.as_str() != run.repo() {
        return RunEnd::Refused(
            "The branch's remote names another repository now: ask again".into(),
        );
    }
    let Some(gh) = Gh::find(root.path()) else {
        return RunEnd::Refused("gh is not installed, or not on PATH".into());
    };
    let Some(state) = state else {
        return RunEnd::Refused("oso has no state directory, so it runs no gh".into());
    };
    let cwd = match crate::git::empty_hooks(state, "gh") {
        Ok(cwd) => cwd,
        Err(why) => return RunEnd::Refused(why),
    };
    if let Err(why) = still_as_asked(&gh, &github, &cwd, run) {
        return RunEnd::Refused(why);
    }
    // A project no longer trusted runs nothing more, even this late.
    if let Err(why) = root.check() {
        return RunEnd::Refused(format!("Not run: {why}"));
    }
    let (args, input) = run.args();
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    let long = match gh.start_long(&github, &words, &cwd, input) {
        Ok(long) => long,
        Err(why) => return RunEnd::Refused(why),
    };
    let line = oso_core::editor::row_text(&log_line(&args));
    let streamed = stream_long(long, format!("{line}\n"), watch, send, false);
    let title = run.title();
    ended_as(streamed, &title, |tail| failed(&title, tail))
}

/// Whether a merge is still of what its question named: the pull request open, into that base.
/// Its head is `gh`'s to check, through `--match-head-commit`.
fn still_as_asked(gh: &Gh, github: &GhRepo, cwd: &Path, run: &PrRun) -> Result<(), String> {
    let PrRun::Merge { number, base, .. } = run else {
        return Ok(());
    };
    let n = number.to_string();
    let words = [
        "pr",
        "view",
        &n,
        "--repo",
        github.as_str(),
        "--json",
        "number,state,baseRefName,headRefOid,headRefName,isCrossRepository",
    ];
    let answer = gh
        .run(github, &words, cwd, MAX_GH, GH_LIMIT)?
        .into_stdout()?;
    let now = read_merge(&answer).ok_or("gh gave an unexpected answer")?;
    if now.cross {
        return Err(format!(
            "#{number} is a fork's pull request: it is not merged"
        ));
    }
    if !now.open {
        return Err(format!("#{number} is not open now: it is not merged"));
    }
    if now.base != *base {
        return Err(format!(
            "#{number} goes into another base now: N m asks again"
        ));
    }
    Ok(())
}
