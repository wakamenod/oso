//! Starts processes for the harness: the app under test, cargo and git.
//! The one place that does (ADR 0044).
//!
//! Must not depend on: the editor or the control socket; it only starts
//! programs.

use std::ffi::OsStr;
use std::process::Command;

/// A command for `program`.
#[expect(
    clippy::disallowed_methods,
    reason = "a benchmark harness that never ships; its use of processes is recorded in ADR 0044"
)]
pub fn command(program: impl AsRef<OsStr>) -> Command {
    Command::new(program)
}
