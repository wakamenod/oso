//! What the runs of git share about paths: names as git writes them, the device and inode of
//! what is at a path, and whether a name may hold secrets (ADR 0044).
//!
//! Must not depend on: `app`, `dispatch` or winit; [`identity_of`] looks at the disk, so it runs
//! on the threads that run git.

use std::ffi::OsStr;
use std::fs::Metadata;
use std::io;
use std::path::Path;

use oso_core::buffer::Buffer;

/// Whether the file at `path` under `top`, the trusted root, may hold secrets, by its name
/// (ADR 0044).
pub(crate) fn may_hold_secrets(top: &Path, path: &[u8]) -> bool {
    let Ok(path) = os(path) else {
        return true;
    };
    Buffer::name_may_hold_secrets(&top.join(path))
}

/// The device and inode `metadata` names, where the system has them.
#[cfg(unix)]
pub(crate) fn identity(metadata: &Metadata) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    Some((metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
pub(crate) fn identity(_: &Metadata) -> Option<(u64, u64)> {
    None
}

/// The device and inode of what is at `path`, not following a link, where the system has
/// them.
pub(super) fn identity_of(path: &Path) -> Option<(u64, u64)> {
    identity(&std::fs::symlink_metadata(path).ok()?)
}

/// A path from git's raw bytes: as they are on Unix, decoded as UTF-8 elsewhere, an invalid
/// byte as U+FFFD. For a path only shown or compared with a listing's, as a worktree's or an
/// added file's, where [`os`], which refuses such a name, would drop the whole record.
pub(crate) fn os_lossy(bytes: &[u8]) -> std::path::PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        std::path::PathBuf::from(OsStr::from_bytes(bytes))
    }
    #[cfg(not(unix))]
    {
        std::path::PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
    }
}

/// `bytes` as a name: on Unix any bytes, elsewhere UTF-8, as git writes paths there.
#[cfg(unix)]
pub(crate) fn os(bytes: &[u8]) -> io::Result<&OsStr> {
    use std::os::unix::ffi::OsStrExt;
    Ok(OsStr::from_bytes(bytes))
}

#[cfg(not(unix))]
pub(crate) fn os(bytes: &[u8]) -> io::Result<&OsStr> {
    std::str::from_utf8(bytes)
        .map(OsStr::new)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "a path that is not UTF-8"))
}
