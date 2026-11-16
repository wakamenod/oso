//! Going on with what is stopped, on the repository's run thread (ADR 0127, step 3a): continuing,
//! skipping and aborting a merge, rebase, pick, revert or `git am`, and telling whether a run
//! left something stopped.
//!
//! Must not depend on: `app`, `dispatch`, winit or the UI thread; these run on `oso-vcs-run`
//! and wait for git.
//!
//! - What is stopped is judged again just before git runs, and [`sequence::refusal`] decides:
//!   a step for something no longer stopped, or a skip or an abort whose question was asked
//!   over another stop, is refused.
//! - Each runs with the repository's own hooks (a continue commits), `GIT_EDITOR=:` and
//!   `GIT_SEQUENCE_EDITOR=:`, so that the message git prepared is kept and no editor opens.
//! - The reads around a run are a read's: the plain git, with no hook, no lazy fetch and no
//!   proxy, and nothing but the git directory's files is read for them.

use super::super::read::{git_dir, judged};
use super::super::sequence::{self, Stop, Want};
use super::stream::stream;
use super::{RunEnd, RunReport, Watch};
use crate::git::Git;

/// Runs `want` with the names `pass` passes on to the hooks, to its end.
pub(super) fn step(
    git: &Git,
    want: &Want,
    pass: &[String],
    watch: &Watch<'_>,
    send: &mut impl FnMut(RunReport) -> bool,
) -> RunEnd {
    if let Some(why) = sequence::refusal(stopped_now(git).as_ref(), want) {
        return RunEnd::Refused(why);
    }
    let (doing, step) = (want.stop.doing, want.step);
    let words = sequence::args(doing, step);
    let title = sequence::title(doing, step);
    let streamed = match stream(&git.with_own_hooks(pass), &words, None, watch, send, false) {
        Ok(streamed) => streamed,
        Err(why) => return RunEnd::Refused(why),
    };
    match streamed.status {
        _ if streamed.stopped => RunEnd::Stopped,
        Ok(status) if status.success() => RunEnd::Done,
        Ok(_) => RunEnd::Failed {
            said: sequence::failed(&title, &streamed.tail, stopped_now(git).is_some()),
            hooks: Vec::new(),
        },
        Err(why) => RunEnd::Failed {
            said: format!("{title} failed: {why}"),
            hooks: Vec::new(),
        },
    }
}

/// What is stopped in the repository now, judged from its files alone, with `git`, a read's.
pub(super) fn stopped_now(git: &Git) -> Option<Stop> {
    git_dir(git).and_then(|dir| judged(&dir))
}
