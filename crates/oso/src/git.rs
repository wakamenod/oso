//! What every run of git in oso shares (ADR 0127, as built): the runner, with oso's own
//! arguments and environment and every read bounded (`run`); object ids (`id`); the root of a
//! trusted project, the only place git starts from (`trust`); names as git writes them and
//! whether one may hold secrets (`path`); the empty directory git is told holds the hooks
//! (`hooks`); runs with no time limit, which the person stops (`long`); `gh`, about the
//! repository a remote names (`gh`); and text from a run made safe to show (`said`). A review (ADR
//! 0122), the worktree jobs (ADR 0121), and a git status and its commits (ADR 0127) use it.
//!
//! Must not depend on: `app`, `dispatch`, winit or the UI thread, nor on what uses it; this
//! runs on the threads that run git, and waits for git.

mod gh;
mod hooks;
mod id;
mod long;
mod path;
mod run;
mod said;
mod trust;

pub(crate) use gh::{Gh, GhAnswer, GhRepo, MAX_GH};
pub(crate) use hooks::empty_hooks;
pub(crate) use id::{Id, trim_line};
pub(crate) use long::{Ender, GRACE, Next};
pub(crate) use path::{identity, may_hold_secrets, os as os_path};
pub(crate) use run::{
    Git, GitError, MAX_SHORT, MAX_STDERR, Output, RUN_LIMIT, check_revision, collect_within,
    for_git, read_at_most, stop,
};
pub(crate) use said::printable;
pub(crate) use trust::TrustedRoot;
