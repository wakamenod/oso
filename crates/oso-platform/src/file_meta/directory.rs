//! A directory opened to list it and to open what it holds relative to itself, following no
//! link on the way, as a project's walk needs (ADR 0114), to make a directory or a new file
//! in it the same way, as a worktree session's copies need (ADR 0121), and to list it with
//! what each entry is, as the directory editor shows it (ADR 0134).
//!
//! Must not depend on: `control`, `privacy`, `process` or `text`.
//!
//! On Unix a [`Directory`] is a descriptor: each one below the first is opened from its parent
//! by one name, refusing a link, and what it holds is listed and opened from that descriptor,
//! so no path is looked up again. On Windows, decided later, it is a path, and each name below
//! it is looked at without following a link at its last name.

use std::ffi::{OsStr, OsString};
use std::fs::{File, Metadata};
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// What an entry of a directory is, without following it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Directory,
    Link,
    /// A device, a pipe, a socket or anything else.
    Other,
}

/// One entry of a directory: its name and what it is.
#[derive(Debug)]
pub struct Entry {
    pub name: OsString,
    pub kind: EntryKind,
}

/// One entry of a directory with what a listing shows of it, read without following it
/// (ADR 0134).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntryMeta {
    pub name: OsString,
    pub kind: EntryKind,
    /// The mode as `ls -l` reads it, its type bits with its permissions, on Unix; 0 elsewhere.
    pub mode: u32,
    /// Its size in bytes: for a link, its target's length in bytes, as `lstat` says.
    pub len: u64,
    pub modified: Option<SystemTime>,
    /// Its owner's and group's ids, on Unix.
    pub owner: Option<(u32, u32)>,
    /// Someone other than this user owns it. Always false where owners are not read.
    pub others: bool,
    /// Its attribute bits on Windows (`FILE_ATTRIBUTE_*`); 0 elsewhere.
    pub attributes: u32,
}

/// An open directory. See the module.
#[derive(Debug)]
pub struct Directory {
    inner: backend::Inner,
}

/// The error for a name that is not one entry of a directory: empty, `.`, `..`, or holding a
/// separator or a NUL.
fn not_one_name() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, "not one name in a directory")
}

/// Whether `name` names one entry of a directory.
fn is_one_name(name: &OsStr) -> bool {
    let bytes = name.as_encoded_bytes();
    !bytes.is_empty()
        && name != "."
        && name != ".."
        && !bytes.contains(&b'/')
        && !bytes.contains(&0)
        && (cfg!(unix) || !bytes.contains(&b'\\'))
}

/// The most bytes of a link's target read.
const MAX_LINK_BYTES: usize = 64 * 1024;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as backend;
#[cfg(not(unix))]
mod other;
#[cfg(not(unix))]
use other as backend;

pub use backend::Entries;

impl Directory {
    /// The directory at `path`, which is followed if it is a link, as the place a walk starts
    /// from is the one the person or the editor named. It touches the disk, so it belongs off
    /// the UI thread.
    ///
    /// # Errors
    ///
    /// If it cannot be opened, or is no directory.
    pub fn open(path: &Path) -> io::Result<Self> {
        backend::open(path).map(|inner| Self { inner })
    }

    /// The directory `name` in this one, opened from it, if it is itself a directory: `None`
    /// for a link, as for a file or anything else, and on macOS for one whose entries a sync
    /// service keeps elsewhere. It touches the disk, so it belongs off the
    /// UI thread.
    ///
    /// # Errors
    ///
    /// If `name` is not one entry's name, or it cannot be opened, with `NotFound` if nothing is
    /// there.
    pub fn open_directory(&self, name: &OsStr) -> io::Result<Option<Self>> {
        backend::open_directory(&self.inner, name).map(|inner| inner.map(|inner| Self { inner }))
    }

    /// The directory `name` in this one, made if missing (mode 0o777 before the umask), and
    /// opened from this one without following a link: `None` if `name` is something else (a
    /// link, a file, a device). It touches the disk, so it belongs off the UI thread.
    ///
    /// # Errors
    ///
    /// If `name` is not one entry's name, or it cannot be made or opened.
    pub fn create_directory(&self, name: &OsStr) -> io::Result<Option<Self>> {
        backend::create_directory(&self.inner, name).map(|inner| inner.map(|inner| Self { inner }))
    }

    /// A new regular file `name` in this one, created only if nothing is there (not even a
    /// dangling link), never through a link, with `mode`'s permission bits (`mode & 0o777` on
    /// Unix; ignored elsewhere), opened for writing. It touches the disk, so it belongs off the
    /// UI thread.
    ///
    /// # Errors
    ///
    /// If `name` is not one entry's name, something is there already (`AlreadyExists`), or it
    /// cannot be created.
    pub fn create_file(&self, name: &OsStr, mode: u32) -> io::Result<File> {
        backend::create_file(&self.inner, name, mode)
    }

    /// Its entries, `.` and `..` left out, each with what it is, not following it. A listing
    /// that fails once ends there. It touches the disk, so it belongs off the UI thread.
    ///
    /// # Errors
    ///
    /// If it cannot be read. An entry that cannot be looked at is an error of its own.
    pub fn entries(&self) -> io::Result<Entries> {
        backend::entries(&self.inner)
    }

    /// Its entries with what a listing shows of each, `.` and `..` left out, read without
    /// following any of them, handed to `each` as they are read, until it says `false`. On
    /// macOS they come in batches from `getattrlistbulk`, elsewhere on Unix one `fstatat` an
    /// entry, and on Windows with the names. An entry that cannot be looked at is handed as an
    /// error, and the listing goes on. It touches the disk, so it belongs off the UI thread.
    ///
    /// # Errors
    ///
    /// If it cannot be read at all, or reading it fails part way.
    pub fn list_with_metadata(
        &self,
        each: &mut dyn FnMut(io::Result<EntryMeta>) -> bool,
    ) -> io::Result<()> {
        backend::list_with_metadata(&self.inner, each)
    }

    /// What is where the entry `name` leads, following it if it is a link, as a listing says
    /// whether a link leads to a directory. It touches the disk, and may wait for another file
    /// system, so it belongs off the UI thread.
    ///
    /// # Errors
    ///
    /// If `name` is not one entry's name, or it cannot be followed, with `NotFound` if nothing
    /// is where it leads.
    pub fn target_kind(&self, name: &OsStr) -> io::Result<EntryKind> {
        backend::target_kind(&self.inner, name)
    }

    /// What the entry `name` is, not following it. It touches the disk, so it belongs off the
    /// UI thread.
    ///
    /// # Errors
    ///
    /// If `name` is not one entry's name, or it cannot be looked at, with `NotFound` if nothing
    /// is there.
    pub fn kind_of(&self, name: &OsStr) -> io::Result<EntryKind> {
        backend::kind_of(&self.inner, name)
    }

    /// Opens the entry `name` for reading if it is itself a regular file, and gives it with its
    /// metadata, read from the open file, as
    /// [`open_regular_here`](super::open_regular_here) does: `None` for a link, a directory, a
    /// device or a pipe, and on macOS for a file whose contents a sync service keeps elsewhere,
    /// which reading would fetch. It is looked at first, so such a thing is not opened at all,
    /// then opened without waiting and without taking a terminal as the controlling one, and
    /// waits again once it is known to be regular. It touches the disk, so it belongs off the
    /// UI thread.
    ///
    /// # Errors
    ///
    /// If `name` is not one entry's name, or it cannot be looked at or opened, with `NotFound`
    /// if nothing is there, or if the file's flags cannot be read or set.
    pub fn open_file(&self, name: &OsStr) -> io::Result<Option<(File, Metadata)>> {
        backend::open_file(&self.inner, name)
    }

    /// The target of the link `name`, as its words say, not looked up. It touches the disk, so
    /// it belongs off the UI thread.
    ///
    /// # Errors
    ///
    /// If `name` is not one entry's name, is no link, or cannot be read, or its target is
    /// longer than 64 KiB.
    pub fn read_link(&self, name: &OsStr) -> io::Result<PathBuf> {
        backend::read_link(&self.inner, name)
    }

    /// Its own metadata: its owner, its device and inode. It touches the disk, so it belongs
    /// off the UI thread.
    ///
    /// # Errors
    ///
    /// If it cannot be read.
    pub fn metadata(&self) -> io::Result<Metadata> {
        backend::metadata(&self.inner)
    }
}

#[cfg(all(test, unix))]
mod tests;
