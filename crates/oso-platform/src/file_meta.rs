//! What a file or directory holds beyond its mode that decides who may reach it. This is the
//! file-system module ADR 0062 expected (ADR 0065). Today that means macOS's access lists: on
//! Linux what a POSIX list grants shows in the mode, and other systems are decided later.
//!
//! Must not depend on: `control`, `privacy`, `process` or `text`.

use std::fs::File;
use std::io;
use std::path::Path;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as backend;
#[cfg(not(target_os = "macos"))]
mod other;
#[cfg(not(target_os = "macos"))]
use other as backend;

#[cfg(all(test, target_os = "macos"))]
pub(crate) use macos::test_list;

/// One entry of an access list, in what the private directory's rules ask of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AccessEntry {
    /// It grants something. Anything else only takes rights away.
    pub(crate) allows: bool,
    /// What it names lets someone add to or remove from a directory, remove the directory itself,
    /// or change its list or owner.
    pub(crate) lets_change: bool,
    /// A directory made in this one takes the entry.
    pub(crate) reaches_new_directories: bool,
}

/// The access list of the directory at `path`, not following a link at its last name, as a link
/// can hold a list of its own. On macOS, having none and a file system with none are both an
/// empty list. Elsewhere this is always empty, as the rules have nothing to ask. It touches the
/// disk, so it belongs off the UI thread where it can be.
///
/// # Errors
///
/// If the list cannot be read, which says nothing of what it holds.
#[cfg_attr(not(unix), allow(dead_code, reason = "used only by the Unix backends"))]
pub(crate) fn directory_access_list(path: &Path) -> io::Result<Vec<AccessEntry>> {
    backend::list_at(path)
}

/// What [`seal`] found on a new file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub struct Sealed {
    had_grant: bool,
}

impl Sealed {
    /// Whether the file had taken from its directory a list granting something, whoever to. It
    /// no longer holds that, but the directory does.
    pub fn had_grant(self) -> bool {
        self.had_grant
    }
}

/// Makes `file`, which this process has just created, grant nothing but what its mode says. Call
/// this before anything goes in. On macOS it removes every access list entry, also from a list
/// its owner may not read, and makes sure none is left. On Linux a new file made with a mode that
/// gives its group nothing already grants nothing through a list, and on other systems nothing is
/// done yet (ADR 0065). It touches the disk, so it belongs off the UI thread.
///
/// # Errors
///
/// If a list cannot be removed. The file must then hold nothing that matters.
pub fn seal(file: &File) -> io::Result<Sealed> {
    backend::seal(file)
}
