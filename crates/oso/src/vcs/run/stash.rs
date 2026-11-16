//! Stashing on the repository's run thread (ADR 0127, step 3b): pushing the changes onto the
//! stash, and applying, popping and dropping a stash found again by its id.
//!
//! Must not depend on: `app`, `dispatch`, winit or the UI thread; these run on `oso-vcs-run`
//! and wait for git.
//!
//! - A stash is named by its commit's id; its `stash@{N}` now is read just before git runs, with
//!   the plain git of a read, so that another worktree's push or drop since moves nothing onto
//!   the wrong one, and one gone is refused (Q5).
//! - Each runs with the repository's own hooks, as a commit does.

use std::ffi::OsStr;

use super::super::stash::{self, Act};
use super::stream::stream;
use super::{RunEnd, RunReport, Watch};
use crate::git::Git;

/// What a stash's run does.
pub(crate) enum StashRun {
    /// Stash the changes, untracked files too if `untracked`, with `message`, empty for git's.
    Push { untracked: bool, message: String },
    /// `act` on the stash whose commit is `id`.
    On { act: Act, id: String },
}

/// Runs `run` with the names `pass` passes on to the hooks, to its end.
pub(super) fn stash(
    git: &Git,
    run: &StashRun,
    pass: &[String],
    watch: &Watch<'_>,
    send: &mut impl FnMut(RunReport) -> bool,
) -> RunEnd {
    let (words, title, act) = match run {
        StashRun::Push { untracked, message } => (
            stash::push_args(*untracked, message),
            "git stash push".to_owned(),
            None,
        ),
        StashRun::On { act, id } => {
            let Some(selector) = now_at(git, id) else {
                return RunEnd::Refused(
                    "That stash is gone, or another took its place: g reads the status again"
                        .into(),
                );
            };
            (
                stash::args(*act, &selector),
                stash::title(*act, &selector),
                Some(*act),
            )
        }
    };
    let streamed = match stream(&git.with_own_hooks(pass), &words, None, watch, send, false) {
        Ok(streamed) => streamed,
        Err(why) => return RunEnd::Refused(why),
    };
    match streamed.status {
        _ if streamed.stopped => RunEnd::Stopped,
        Ok(status) if status.success() => RunEnd::Done,
        Ok(_) => RunEnd::Failed {
            said: match act {
                Some(act) => stash::failed(act, &title, &streamed.tail),
                None => super::super::remote::failed(&title, &streamed.tail),
            },
            hooks: Vec::new(),
        },
        Err(why) => RunEnd::Failed {
            said: format!("{title} failed: {why}"),
            hooks: Vec::new(),
        },
    }
}

/// Where the stash whose commit is `id` is now, as `stash@{N}`, read with `git`, a read's.
fn now_at(git: &Git, id: &str) -> Option<String> {
    let words = ["stash", "list", "-z", "--no-show-signature", "--format=%H"].map(OsStr::new);
    let out = git.run(&words, &[], crate::vcs::history::MAX_LOG).ok()?;
    stash::find(&out, id)
}
