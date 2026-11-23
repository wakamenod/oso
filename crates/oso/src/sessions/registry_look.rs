//! One look at a hosted session's own entry in Claude Code's registry, on a short thread of
//! its own, `oso-session-idle`, before an agent's `session_archive` stops it (ADR 0179):
//! `registry` reads, but has no threads of its own, and `sessions::tools::registry_idle`
//! judges what this found.
//!
//! Must not depend on: the UI or `app`; the app gets what was read through `send`.

use std::io;
use std::thread;

use oso_platform::process;

use super::registry::{self, Entry, Live};

/// What a look found: process `pid`'s entry, and whether its process is the one it names.
pub type Found = Option<(Entry, Live)>;

/// Reads process `pid`'s registry entry, with whether its process is the one it names, on
/// `oso-session-idle`, and calls `send` with it: `None` when the registry's directory is not
/// known or the entry cannot be read.
///
/// # Errors
///
/// If no thread starts.
pub fn spawn_look(pid: u32, send: impl FnOnce(Found) + Send + 'static) -> io::Result<()> {
    thread::Builder::new()
        .name("oso-session-idle".into())
        .spawn(move || send(look(pid)))
        .map(drop)
}

/// Process `pid`'s registry entry, as [`spawn_look`] sends it.
fn look(pid: u32) -> Found {
    let entry = registry::read_pid(&registry::directory()?, pid)?;
    let live = registry::liveness(&entry, process::start_time);
    Some((entry, live))
}
