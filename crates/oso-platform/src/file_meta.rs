//! What a file or directory holds beyond its mode that decides who may reach it, which today
//! means macOS's access lists (ADR 0065). On Linux a list's effect shows in the mode, and other
//! systems are recorded there as later, so elsewhere this finds nothing and does nothing.
//!
//! Must not depend on: `control`, `privacy`, `process` or `text`.

use std::fs::File;
use std::io;
use std::path::Path;

#[cfg(target_os = "macos")]
pub(crate) mod macos;
#[cfg(target_os = "macos")]
use macos as backend;
#[cfg(not(target_os = "macos"))]
mod other;
#[cfg(not(target_os = "macos"))]
use other as backend;

/// One entry of an access list, in what the private directory's rules ask of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ListEntry {
    /// It grants something. Anything else only takes rights away.
    pub(crate) allows: bool,
    /// What it names lets someone add to or remove from a directory, or change who may.
    pub(crate) changes_contents: bool,
    /// A directory made in this one takes the entry.
    pub(crate) reaches_new_directories: bool,
}

/// The access list of the directory at `path`, not following a link at its last name. None at
/// all, and a file system that has no such thing, are both an empty list. It touches the disk, so
/// it belongs off the UI thread.
///
/// # Errors
///
/// If the list cannot be read, which says nothing of what it holds.
#[cfg_attr(not(unix), allow(dead_code, reason = "used only by the Unix backends"))]
pub(crate) fn directory_list(path: &Path) -> io::Result<Vec<ListEntry>> {
    backend::list_at(path)
}

/// What [`seal`] found on a new file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub struct Sealed {
    grants_others: bool,
}

impl Sealed {
    /// Whether the file had taken from its directory a list granting someone something. It no
    /// longer holds it, but the directory does.
    pub fn grants_others(self) -> bool {
        self.grants_others
    }
}

/// Removes every access list entry from `file`, which this process has just created, and makes
/// sure none is left, so that only its mode decides who may reach it. Call this before anything
/// goes in. It touches the disk, so it belongs off the UI thread.
///
/// # Errors
///
/// If a list cannot be read or removed. The file must then hold nothing that matters.
pub fn seal(file: &File) -> io::Result<Sealed> {
    backend::seal(file)
}
