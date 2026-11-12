//! Agent sessions (ADR 0121): a hosted `claude` in an oso terminal, and what oso tells the
//! person about it. This holds its states so far: the rules that read a session's state from
//! its screen, the tracker that turns their verdicts into the taxonomy, and the `oso-sessions`
//! worker that runs both off the UI thread.
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

pub mod engine;
pub mod explain;
pub mod rules;
pub mod screen;
pub mod state;
pub mod worker;

#[cfg(test)]
mod fuzz_tests;
#[cfg(test)]
mod screens_tests;
