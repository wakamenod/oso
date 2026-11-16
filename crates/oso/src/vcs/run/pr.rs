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
//!   person stops it with `C-g`, and its output goes to the log as it comes.

use std::path::Path;

use super::super::pr::create::PrRun;
use super::super::pr::work::branch_repo;
use super::super::remote;
use super::stream::{ended_as, stream_long};
use super::{RunEnd, RunReport, Watch};
use crate::git::{Gh, Git, TrustedRoot};

/// Runs `run` about the repository at `root`, `gh`'s directory under `state`, to its end.
pub(super) fn run(
    git: &Git,
    (root, state): (&TrustedRoot, Option<&Path>),
    run: &PrRun,
    watch: &Watch<'_>,
    send: &mut impl FnMut(RunReport) -> bool,
) -> RunEnd {
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
    let github = match branch_repo(git) {
        Ok(github) => github,
        Err(why) => return RunEnd::Refused(why),
    };
    if github.as_str() != run.repo() {
        return RunEnd::Refused(
            "The branch's remote names another repository now: ask again".into(),
        );
    }
    let (args, input) = run.args();
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    let long = match gh.start_long(&github, &words, &cwd, input) {
        Ok(long) => long,
        Err(why) => return RunEnd::Refused(why),
    };
    let line = format!("$ gh {}\n", oso_core::editor::row_text(&args.join(" ")));
    let streamed = stream_long(long, line, watch, send, false);
    let title = run.title();
    ended_as(streamed, &title, |tail| remote::failed(&title, tail))
}
