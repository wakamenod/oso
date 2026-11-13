//! Agent sessions (ADR 0121): a hosted `claude` in an oso terminal, and what oso tells the
//! person about it: starting, resuming and remembering sessions, the bridge's session, the
//! rules that read a session's state from its screen, the tracker that turns their verdicts into
//! the taxonomy, and the `oso-sessions` worker that runs them off the UI thread.
//!
//! Must not depend on: `app`, `dispatch`, `commands` or `config`; the app feeds the worker and
//! carries out what it says, and `config`'s thread compiles the local rules with `rules`.
//!
//! - `screen`: what a look at a terminal found, and the regions rules read.
//! - `rules`: the rules files, bundled and local, compiled and bounded.
//! - `engine`: the rules over one look.
//! - `state`: the taxonomy and the tracker, with the debounce and the grace period.
//! - `explain`: what `session-explain` and `session-explain-file` show.
//! - `worker`: the thread.
//! - `identity`: a session's id and name.
//! - `command`: the `claude` command line, and `launch`: the login shell that runs it.
//! - `registry`: Claude Code's registry of running sessions, and whether each is live.
//! - `record`: the sessions oso remembers, in its state directory.
//! - `hosted`: the table of the sessions oso hosts.
//! - `bridge`: whether a connection is a hosted session's bridge.
//! - `start`: getting a session ready on a thread of its own.
//! - `worktree`: worktree sessions, their briefs, the list and safe removal.
//! - `tools`: the agents' session tools: reach, questions, the loop guard and refusals.

pub mod bridge;
pub mod command;
pub mod engine;
pub mod explain;
pub mod hosted;
pub mod identity;
pub mod launch;
pub mod record;
pub mod registry;
pub mod rules;
pub mod screen;
pub mod start;
pub mod state;
pub mod tools;
pub mod worker;
pub mod worktree;

#[cfg(test)]
mod fuzz_tests;
#[cfg(test)]
mod screens_tests;
