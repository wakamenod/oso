//! Reading a review's new side from the work tree (ADR 0122), as project search's preview
//! reads a file: from the work tree's top a directory at a time, following no link, regular
//! files only, waiting for nothing.
//!
//! Must not depend on: `app`, `dispatch` or anything that runs git; this reads the file system
//! on the review's thread.
//!
//! - A link is never followed: its side is its target's text, as git stores a link.
//! - A file past [`MAX_BLOB`] is not read; its size is enough to say it is too large.
//! - What a file is, by its device, inode, size and modification time, is its stamp, so a
//!   file whose stamp did not change is not read again.

use std::ffi::OsStr;
use std::fs::{File, Metadata};
use std::io::{self, Read};
use std::path::Path;
use std::time::UNIX_EPOCH;

use oso_core::buffer::Buffer;
use oso_platform::file_meta::{Directory, EntryKind};

use super::batch::MAX_BLOB;

/// The most directories a path may go down.
const MAX_DEPTH: usize = 128;
/// How many leading bytes decide that a side is binary, as git decides it.
pub(crate) const BINARY_PROBE: usize = 8000;

/// What a regular file is, so that a change of it is seen without reading it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Stamp {
    device: u64,
    inode: u64,
    pub(crate) size: u64,
    modified: u128,
}

/// What is at a path in the work tree.
pub(crate) enum Found {
    /// A regular file, open, with its stamp.
    File(File, Stamp),
    /// A link, and the bytes of its target.
    Link(Vec<u8>),
    /// Nothing, or something that is no file or link: a directory, a pipe, a device.
    Nothing,
}

/// Looks at `path`, `/`-separated as git spells it, under `top`, opening a regular file but
/// reading nothing from it.
pub(crate) fn find(top: &Directory, path: &[u8]) -> io::Result<Found> {
    let names: Vec<&[u8]> = path.split(|&byte| byte == b'/').collect();
    let Some((name, directories)) = names.split_last() else {
        return Ok(Found::Nothing);
    };
    if directories.len() > MAX_DEPTH {
        return Ok(Found::Nothing);
    }
    let mut below: Option<Directory> = None;
    for directory in directories {
        let here = below.as_ref().unwrap_or(top);
        match here.open_directory(os(directory)?) {
            Ok(Some(next)) => below = Some(next),
            Ok(None) => return Ok(Found::Nothing),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Found::Nothing),
            Err(e) => return Err(e),
        }
    }
    let here = below.as_ref().unwrap_or(top);
    let name = os(name)?;
    let kind = match here.kind_of(name) {
        Ok(kind) => kind,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Found::Nothing),
        Err(e) => return Err(e),
    };
    match kind {
        EntryKind::Link => Ok(Found::Link(target_bytes(&here.read_link(name)?))),
        EntryKind::File => Ok(match here.open_file(name)? {
            Some((file, metadata)) => Found::File(file, stamp(&metadata)),
            None => Found::Nothing,
        }),
        EntryKind::Directory | EntryKind::Other => Ok(Found::Nothing),
    }
}

/// What reading a file gave.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Got {
    Bytes(Vec<u8>),
    /// It holds more than [`MAX_BLOB`] bytes: this many, or more.
    TooLarge(u64),
}

/// Reads `file`, whose stamp says `size`, keeping at most [`MAX_BLOB`] bytes.
pub(crate) fn read(file: File, size: u64) -> io::Result<Got> {
    if size > MAX_BLOB {
        return Ok(Got::TooLarge(size));
    }
    let mut bytes = Vec::with_capacity(usize::try_from(size).unwrap_or(0));
    file.take(MAX_BLOB + 1).read_to_end(&mut bytes)?;
    let read = bytes.len() as u64;
    Ok(if read > MAX_BLOB {
        Got::TooLarge(read)
    } else {
        Got::Bytes(bytes)
    })
}

/// Whether `bytes` read as binary: a NUL in the first [`BINARY_PROBE`] of them.
pub(crate) fn is_binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(BINARY_PROBE)].contains(&0)
}

/// Whether the file at `path` under `top`, or under `root` (the project as it was named),
/// may hold secrets, by its name (ADR 0044).
pub(crate) fn may_hold_secrets(top: &Path, root: &Path, path: &[u8]) -> bool {
    let Ok(path) = os(path) else {
        return true;
    };
    Buffer::name_may_hold_secrets(&top.join(path))
        || Buffer::name_may_hold_secrets(&root.join(path))
}

fn stamp(metadata: &Metadata) -> Stamp {
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_nanos());
    let (device, inode) = identity(metadata);
    Stamp {
        device,
        inode,
        size: metadata.len(),
        modified,
    }
}

#[cfg(unix)]
fn identity(metadata: &Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    (metadata.dev(), metadata.ino())
}

#[cfg(not(unix))]
fn identity(_: &Metadata) -> (u64, u64) {
    (0, 0)
}

/// `bytes` as a name: on Unix any bytes, elsewhere UTF-8, as git writes paths there.
#[cfg(unix)]
fn os(bytes: &[u8]) -> io::Result<&OsStr> {
    use std::os::unix::ffi::OsStrExt;
    Ok(OsStr::from_bytes(bytes))
}

#[cfg(not(unix))]
fn os(bytes: &[u8]) -> io::Result<&OsStr> {
    std::str::from_utf8(bytes)
        .map(OsStr::new)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "a path that is not UTF-8"))
}

/// A link's target as git stores it: its bytes, with `/` between names.
#[cfg(unix)]
fn target_bytes(target: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    target.as_os_str().as_bytes().to_vec()
}

#[cfg(not(unix))]
fn target_bytes(target: &Path) -> Vec<u8> {
    target.to_string_lossy().replace('\\', "/").into_bytes()
}

#[cfg(test)]
mod tests;
