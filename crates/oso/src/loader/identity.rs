//! What a file is, by its metadata: what a save records of it, and whether two sets of
//! metadata are of one file (ADR 0029).
//!
//! Must not depend on: the editor's state or `app`; these only compare what the system says.

use std::fs::Metadata;
use std::io;

use oso_core::buffer::FileStamp;

/// What a save records of a file, from its metadata, with its device and inode. Loading and saving must agree on
/// it, or every save would report a conflict.
///
/// # Errors
///
/// On a system or file system that keeps no modification time.
pub fn stamp_of(metadata: &Metadata) -> io::Result<FileStamp> {
    #[cfg(unix)]
    let mode = std::os::unix::fs::PermissionsExt::mode(&metadata.permissions());
    #[cfg(not(unix))]
    let mode = 0;
    #[cfg(unix)]
    let identity = {
        use std::os::unix::fs::MetadataExt;
        Some((metadata.dev(), metadata.ino()))
    };
    // `std` keeps a file's identity unstable here.
    #[cfg(not(unix))]
    let identity = None;
    Ok(FileStamp {
        modified: metadata.modified()?,
        len: metadata.len(),
        mode,
        identity,
    })
}

/// Whether two sets of metadata are of one file: its device and inode.
#[cfg(unix)]
pub(crate) fn same_file(a: &Metadata, b: &Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    a.dev() == b.dev() && a.ino() == b.ino()
}

/// Whether two sets of metadata are known to be of one file. Unlike [`same_file`], this does
/// not guess: where the system cannot say, two files are two.
#[cfg(unix)]
pub(crate) fn surely_same_file(a: &Metadata, b: &Metadata) -> bool {
    same_file(a, b)
}

/// `std` keeps a file's identity unstable here, so nothing can be sure.
#[cfg(not(unix))]
pub(crate) fn surely_same_file(_: &Metadata, _: &Metadata) -> bool {
    false
}

/// `std` keeps a file's identity unstable here, so this goes by what a save compares.
#[cfg(not(unix))]
pub(crate) fn same_file(a: &Metadata, b: &Metadata) -> bool {
    same_stamp(a, b)
}

/// Whether two sets of metadata show one modification time and size.
#[cfg(any(not(unix), test))]
pub(super) fn same_stamp(a: &Metadata, b: &Metadata) -> bool {
    match (stamp_of(a), stamp_of(b)) {
        (Ok(a), Ok(b)) => a.same_content(b),
        _ => false,
    }
}
