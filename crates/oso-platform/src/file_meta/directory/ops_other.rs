//! What the directory editor does to files where a directory is a path, which today means
//! Windows (ADR 0134, step 1b): each name is looked at without following a link at its last
//! name, then renamed, removed or copied by its path.
//!
//! Must not depend on: `control`, `privacy`, `process` or `text`.

use std::ffi::OsStr;
use std::io;

use super::{Inner, joined};

pub(in crate::file_meta::directory) fn identity(_: &Inner) -> io::Result<Option<(u64, u64)>> {
    // `std` keeps a file's identity unstable here.
    Ok(None)
}

pub(in crate::file_meta::directory) fn rename_to(
    inner: &Inner,
    name: &OsStr,
    to: &Inner,
    to_name: &OsStr,
    replace: bool,
) -> io::Result<()> {
    let (from, to) = (joined(inner, name)?, joined(to, to_name)?);
    if !replace && std::fs::symlink_metadata(&to).is_ok() {
        return Err(io::ErrorKind::AlreadyExists.into());
    }
    std::fs::rename(from, to)
}

/// Removes the entry at `path`, never what a link at it leads to: a link to a directory, or a
/// junction, which Windows removes as a directory, is removed as the link it is.
pub(in crate::file_meta::directory) fn remove(inner: &Inner, name: &OsStr) -> io::Result<()> {
    let path = joined(inner, name)?;
    let metadata = std::fs::symlink_metadata(&path)?;
    if metadata.is_dir() || is_directory_link(&metadata) {
        std::fs::remove_dir(path)
    } else {
        std::fs::remove_file(path)
    }
}

/// Whether `metadata`, read without following, is of a link or a junction to a directory, which
/// is removed as a directory is and listed as the link it is.
#[cfg(windows)]
fn is_directory_link(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
    metadata.file_type().is_symlink() && metadata.file_attributes() & FILE_ATTRIBUTE_DIRECTORY != 0
}

#[cfg(not(windows))]
fn is_directory_link(_: &std::fs::Metadata) -> bool {
    false
}

pub(in crate::file_meta::directory) fn make_directory(
    inner: &Inner,
    name: &OsStr,
    _mode: u32,
) -> io::Result<Option<Inner>> {
    super::create_directory(inner, name)
}

pub(in crate::file_meta::directory) fn copy_file_to(
    inner: &Inner,
    name: &OsStr,
    to: &Inner,
    to_name: &OsStr,
    replace: bool,
) -> io::Result<()> {
    let (from, to) = (joined(inner, name)?, joined(to, to_name)?);
    if !std::fs::symlink_metadata(&from)?.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file here",
        ));
    }
    if !replace && std::fs::symlink_metadata(&to).is_ok() {
        return Err(io::ErrorKind::AlreadyExists.into());
    }
    std::fs::copy(from, to).map(|_| ())
}

pub(in crate::file_meta::directory) fn copy_link_to(
    _: &Inner,
    _: &OsStr,
    _: &Inner,
    _: &OsStr,
    _: bool,
) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "links are not copied here yet",
    ))
}
