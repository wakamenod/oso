//! Worktree sessions (ADR 0121, "Worktree sessions"): making a git worktree for a new session
//! with a brief, copying `.worktreeinclude`'s files into it, listing a repository's linked
//! worktrees, removing one only when nothing in it would be lost, and the hook that steers
//! Claude away from its own worktree tools. Every `git` and `gh` call runs on the
//! `oso-sessions-git` threads, through `oso-platform::process`, only for a trusted project
//! whose main repository is trusted too.
//!
//! Must not depend on: `app`, `dispatch` or the UI thread; the app asks for jobs and
//! carries out what they report.
//!
//! - `slug`: a branch's directory name, and where its worktree goes.
//! - `porcelain`: what git prints, parsed.
//! - `brief`: the brief's header and hand-off facts.
//! - `transcript`: the starting session's transcript, and the files it touched.
//! - `include`: `.worktreeinclude`'s files, matched and copied.
//! - `steer`: `oso hook deny-worktree`, and the `--settings` that installs it for a session.
//! - `git`: git and `gh` for a repository and its linked worktrees.
//! - `facts`: what a job reads of a repository once, for every worktree it looks at.
//! - `merged`: whether a branch is merged (check 5).
//! - `checks`: what is known of a worktree, and the five checks of a removal.
//! - `create`: making a worktree for a session.
//! - `pool`: the `oso-sessions-git` threads, and `jobs`: the jobs the app asks for on them.
//! - `list`: the text of `*Worktrees*`.

pub mod brief;
pub mod checks;
pub mod create;
pub mod facts;
pub mod git;
pub mod include;
pub mod jobs;
pub mod list;
pub mod merged;
pub mod pool;
pub mod porcelain;
pub mod slug;
pub mod steer;
pub mod transcript;

#[cfg(all(test, unix))]
mod tests;
