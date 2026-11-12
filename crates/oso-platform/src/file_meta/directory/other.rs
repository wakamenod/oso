//! A directory as a path where opening relative to one is decided later, which today means
//! Windows: each name below it is looked at without following a link at its last name, but
//! what is reached is not held, so a directory replaced between two looks is not noticed.
//!
//! Must not depend on: `control`, `privacy`, `process` or `text`.

use std::ffi::OsStr;
use std::fs::{File, FileType, Metadata};
use std::io;
use std::path::{Path, PathBuf};

use super::{Entry, EntryKind, MAX_LINK_BYTES, is_one_name, not_one_name};

/// An open directory: its path.
pub(super) type Inner = PathBuf;

/// The kind a file type says.
fn kind_of_type(kind: FileType) -> EntryKind {
    if kind.is_symlink() {
        EntryKind::Link
    } else if kind.is_dir() {
        EntryKind::Directory
    } else if kind.is_file() {
        EntryKind::File
    } else {
        EntryKind::Other
    }
}

/// The path of `name` in `inner`, if it is one entry's name.
fn joined(inner: &Inner, name: &OsStr) -> io::Result<PathBuf> {
    if !is_one_name(name) {
        return Err(not_one_name());
    }
    Ok(inner.join(name))
}

pub(super) fn open(path: &Path) -> io::Result<Inner> {
    if !std::fs::metadata(path)?.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            "not a directory",
        ));
    }
    Ok(path.to_path_buf())
}

pub(super) fn open_directory(inner: &Inner, name: &OsStr) -> io::Result<Option<Inner>> {
    let path = joined(inner, name)?;
    let kind = std::fs::symlink_metadata(&path)?.file_type();
    Ok((kind.is_dir() && !kind.is_symlink()).then_some(path))
}

pub(super) fn kind_of(inner: &Inner, name: &OsStr) -> io::Result<EntryKind> {
    let path = joined(inner, name)?;
    Ok(kind_of_type(std::fs::symlink_metadata(path)?.file_type()))
}

pub(super) fn open_file(inner: &Inner, name: &OsStr) -> io::Result<Option<(File, Metadata)>> {
    super::super::open_regular_here(&joined(inner, name)?)
}

pub(super) fn read_link(inner: &Inner, name: &OsStr) -> io::Result<PathBuf> {
    let target = std::fs::read_link(joined(inner, name)?)?;
    if target.as_os_str().len() > MAX_LINK_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "a link's target is too long",
        ));
    }
    Ok(target)
}

pub(super) fn metadata(inner: &Inner) -> io::Result<Metadata> {
    std::fs::symlink_metadata(inner)
}

pub(super) fn entries(inner: &Inner) -> io::Result<Entries> {
    std::fs::read_dir(inner).map(Entries)
}

/// The entries of a directory, as [`Directory::entries`](super::Directory::entries) gives them.
#[derive(Debug)]
pub struct Entries(std::fs::ReadDir);

impl Iterator for Entries {
    type Item = io::Result<Entry>;

    fn next(&mut self) -> Option<Self::Item> {
        let entry = self.0.next()?;
        Some(entry.and_then(|entry| {
            let kind = kind_of_type(entry.file_type()?);
            Ok(Entry {
                name: entry.file_name(),
                kind,
            })
        }))
    }
}
