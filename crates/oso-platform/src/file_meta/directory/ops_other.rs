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

pub(in crate::file_meta::directory) fn remove(inner: &Inner, name: &OsStr) -> io::Result<()> {
    let path = joined(inner, name)?;
    let kind = std::fs::symlink_metadata(&path)?.file_type();
    if kind.is_dir() {
        std::fs::remove_dir(path)
    } else {
        std::fs::remove_file(path)
    }
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
