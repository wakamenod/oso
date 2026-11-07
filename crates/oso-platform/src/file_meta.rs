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
    /// What it names is what making, renaming and removing a file in a directory needs.
    pub(crate) names_writing: bool,
    /// What it names is what going through a directory needs.
    pub(crate) names_searching: bool,
    /// What it names is what making a directory in a directory needs.
    pub(crate) names_making_directories: bool,
    /// What it names is reading the list, which a file or directory that takes it could then
    /// not have read.
    pub(crate) names_reading_the_list: bool,
    /// It counts for the directory itself, and not only for what is made in it.
    pub(crate) applies_here: bool,
    /// A file made in this directory takes the entry.
    pub(crate) reaches_new_files: bool,
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

/// Makes `file`, which this process has just created, grant nothing but what its mode says. Call
/// this before anything goes in. On macOS it removes every access list entry and makes sure none
/// is left. On Linux a new file made with a mode that gives its group nothing already grants
/// nothing through a list, and on other systems nothing is done yet (ADR 0065). It touches the disk,
/// so it belongs off the UI thread.
///
/// # Errors
///
/// If a list cannot be removed, or what the file took from its directory granted anything or could
/// not be read: the directory is then no place for what goes in, whatever it said when it was
/// checked. The list is gone from the file if it could be removed, so the file can be, too.
pub fn seal(file: &File) -> io::Result<()> {
    backend::seal(file)
}
