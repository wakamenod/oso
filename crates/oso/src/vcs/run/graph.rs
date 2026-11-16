//! A graph's runs on the repository's run thread (ADR 0127, step 4b): making a tag, cherry-picking
//! commits and reverting one.
//!
//! Must not depend on: `app`, `dispatch`, winit or the UI thread; these run on `oso-vcs-run`
//! and wait for git.
//!
//! - Each runs with the repository's own hooks, as a commit does; a tag's name passes the
//!   branch's filter, and every id must be a whole one, as a graph's line gives it.
//! - A pick and a revert are refused while something is stopped, judged here (Q6); one that
//!   stops on a conflict says so, and the status's `C-c C-c` and `C-c C-k` go on with it.

use super::super::graph::GraphRun;
use super::super::remote::{self, check_branch};
use super::super::sequence;
use super::branch::check_id;
use super::sequence::refused_while_stopped;
use super::sequence::stopped_now;
use super::stream::{ended_as, stream};
use super::{RunEnd, RunReport, Watch};
use crate::git::Git;

/// Runs `run` with the names `pass` passes on to the hooks, to its end.
pub(super) fn run(
    git: &Git,
    run: &GraphRun,
    pass: &[String],
    watch: &Watch<'_>,
    send: &mut impl FnMut(RunReport) -> bool,
) -> RunEnd {
    if let Err(why) = check(git, run) {
        return RunEnd::Refused(why);
    }
    let (words, input) = run.args();
    let title = run.title();
    let streamed = match stream(&git.with_own_hooks(pass), &words, input, watch, send, false) {
        Ok(streamed) => streamed,
        Err(why) => return RunEnd::Refused(why),
    };
    let stops = !matches!(run, GraphRun::Tag { .. });
    ended_as(streamed, &title, |tail| {
        if stops {
            sequence::failed(&title, tail, stopped_now(git).is_some())
        } else {
            remote::failed(&title, tail)
        }
    })
}

/// Why `run` may not go to git, if it may not.
fn check(git: &Git, run: &GraphRun) -> Result<(), String> {
    for id in run.ids() {
        check_id(id)?;
    }
    match run {
        GraphRun::Tag { name, .. } => check_branch(name).map_err(|_| {
            format!(
                "Not a tag's name git takes: {}",
                oso_core::editor::row_text(name)
            )
        }),
        GraphRun::Pick { .. } | GraphRun::Revert { .. } => refused_while_stopped(git),
    }
}
