//! What the runs of git share about paths: names as git writes them, a path under the top as
//! git names it, the device and inode of what is at a path, and whether a name may hold secrets
//! (ADR 0044).
//!
//! Must not depend on: `app`, `dispatch` or winit; [`identity_of`] looks at the disk, so it runs
//! on the threads that run git.

use std::ffi::OsStr;
use std::fs::Metadata;
use std::io;
use std::path::Path;

use oso_core::buffer::{Buffer, Verdict};

/// Whether the file at `path` under `top`, the trusted root, may hold secrets, by its name
/// (ADR 0044).
pub(crate) fn may_hold_secrets(top: &Path, path: &[u8]) -> bool {
    verdict(top, path).is_withheld()
}

/// The verdict on the file at `path` under `top`, the trusted root, by its name (ADR 0044): a
/// name that is no path here is [`Verdict::Unresolved`], which withholds it too.
pub(crate) fn verdict(top: &Path, path: &[u8]) -> Verdict {
    match os(path) {
        Ok(path) => Buffer::judge_name(&top.join(path)),
        Err(_) => Verdict::Unresolved,
    }
}

/// `relative`, a path under a work tree's top, as git names it: its names joined by `/`, each
/// as its bytes on Unix and as UTF-8 elsewhere, as git writes paths there. None for one that
/// leaves the top, names nothing, or, off Unix, is not UTF-8.
pub(crate) fn name_of(relative: &Path) -> Option<Vec<u8>> {
    let mut name = Vec::new();
    for component in relative.components() {
        let part = match component {
            std::path::Component::Normal(part) => part,
            std::path::Component::CurDir => continue,
            _ => return None,
        };
        if !name.is_empty() {
            name.push(b'/');
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            name.extend_from_slice(part.as_bytes());
        }
        #[cfg(not(unix))]
        name.extend_from_slice(part.to_str()?.as_bytes());
    }
    (!name.is_empty()).then_some(name)
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

#[cfg(test)]
mod name_tests {
    use std::path::Path;

    use super::name_of;

    #[test]
    fn a_path_under_the_top_is_named_as_git_names_it() {
        assert_eq!(
            name_of(Path::new("src/a.rs")).as_deref(),
            Some(&b"src/a.rs"[..])
        );
        assert_eq!(name_of(Path::new("./a.rs")).as_deref(), Some(&b"a.rs"[..]));
        assert_eq!(name_of(Path::new("../a.rs")), None);
        assert_eq!(name_of(Path::new("")), None);
    }
}
