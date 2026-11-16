//! Stashing on the repository's run thread (ADR 0127, step 3b): pushing the changes onto the
//! stash, and applying, popping and dropping a stash found again by its id.
//!
//! Must not depend on: `app`, `dispatch`, winit or the UI thread; these run on `oso-vcs-run`
//! and wait for git.
//!
//! - A stash is named by its commit's id. An apply gives git that id; a pop or a drop reads
//!   git's own `stash@{N}` for it just before git runs, with the plain git of a read, as
//!   another worktree may have pushed or dropped one since; one gone is refused (Q5).
//! - Each runs with the repository's own hooks, as a commit does.

use std::ffi::OsStr;

use super::super::stash::{self, Act};
use super::stream::{ended_as, stream};
use super::{RunEnd, RunReport, Watch};
use crate::git::Git;

/// What a stash's run does.
pub(crate) enum StashRun {
    /// Stash the changes, untracked files too if `untracked`, with `message`, empty for git's.
    Push { untracked: bool, message: String },
    /// `act` on the stash whose commit is `id`.
    Act { act: Act, id: String },
}

/// Runs `run` with the names `pass` passes on to the hooks, to its end.
pub(super) fn run(
    git: &Git,
    run: &StashRun,
    pass: &[String],
    watch: &Watch<'_>,
    send: &mut impl FnMut(RunReport) -> bool,
) -> RunEnd {
    let (words, title, act) = match run {
        StashRun::Push { untracked, message } => (
            stash::push_args(*untracked, message),
            "git stash push",
            None,
        ),
        StashRun::Act { act, id } => {
            // An apply takes the id itself; a pop or a drop only git's selector for it now.
            let target = if *act == Act::Apply {
                Some(id.clone())
            } else {
                now_at(git, id)
            };
            let Some(target) = target else {
                return RunEnd::Refused("That stash is gone: g reads the status again".into());
            };
            (stash::args(*act, &target), stash::title(*act), Some(*act))
        }
    };
    let streamed = match stream(&git.with_own_hooks(pass), &words, None, watch, send, false) {
        Ok(streamed) => streamed,
        Err(why) => return RunEnd::Refused(why),
    };
    ended_as(streamed, title, |tail| match act {
        Some(act) => stash::failed(act, title, tail),
        None => super::super::remote::failed(title, tail),
    })
}

/// git's own selector for the stash whose id is `id`, read with `git`, a read's.
fn now_at(git: &Git, id: &str) -> Option<String> {
    let words = stash::find_args();
    let words: Vec<&OsStr> = words.iter().map(OsStr::new).collect();
    let out = git.run(&words, &[], crate::vcs::history::MAX_LOG).ok()?;
    stash::find(&out, id)
}
