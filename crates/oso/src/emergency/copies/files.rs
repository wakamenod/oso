//! What may be read or removed in the directory: the forms of the names in it, and the checks
//! a file must pass before anything opens it (ADR 0064).
//!
//! Must not depend on: `app`, `dispatch` or winit; these are plain functions of paths.

use std::fs::{self, File, Metadata};
use std::io;
use std::path::Path;
use std::time::SystemTime;

use oso_platform::privacy::is_this_users;

use super::LEFT_BEHIND;
use crate::loader::same_file;
use crate::saver::TEMPORARY_PREFIX;

/// Whether `name` has the form `<16 hex>-<digits>-<digits>-<digits>`, and if so the seconds.
pub(super) fn stem_seconds(name: &str) -> Option<u64> {
    let mut parts = name.split('-');
    let hash = parts.next()?;
    let seconds = parts.next()?;
    let process = parts.next()?;
    let attempt = parts.next()?;
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    let hex = hash.len() == 16
        && hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    let fits = hex && digits(seconds) && digits(process) && digits(attempt);
    (fits && parts.next().is_none())
        .then(|| seconds.parse().ok())
        .flatten()
}

/// Whether `name` has the exact form of a temporary file.
pub(super) fn is_temporary(name: &str) -> bool {
    let middle = name
        .strip_prefix(TEMPORARY_PREFIX)
        .and_then(|rest| rest.strip_suffix(".tmp"));
    middle.is_some_and(|middle| {
        let mut parts = middle.split('-');
        let digits = |part: Option<&str>| {
            part.is_some_and(|part| {
                !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())
            })
        };
        digits(parts.next())
            && digits(parts.next())
            && digits(parts.next())
            && parts.next().is_none()
    })
}

/// A regular file of this user's with one name, by what `symlink_metadata` says, before
/// anything opens it, as opening a pipe blocks. A hard link is a name for some other file's text.
pub(super) fn plain_own_file(path: &Path) -> Option<Metadata> {
    let metadata = fs::symlink_metadata(path).ok()?;
    let plain = metadata.is_file() && is_this_users(&metadata) && one_name(&metadata);
    plain.then_some(metadata)
}

#[cfg(unix)]
fn one_name(metadata: &Metadata) -> bool {
    std::os::unix::fs::MetadataExt::nlink(metadata) == 1
}

#[cfg(not(unix))]
fn one_name(_: &Metadata) -> bool {
    true
}

/// Opens a copy's text to read, by the same checks.
pub fn open_copy(path: &Path) -> io::Result<File> {
    open_checked(path).map(|(file, _)| file)
}

/// Opens `path` if it is [`plain_own_file`], and is still that file once open.
pub(in crate::emergency) fn open_checked(path: &Path) -> io::Result<(File, Metadata)> {
    let not_plain = || io::Error::other("it is not a plain file of this user's");
    let looked = plain_own_file(path).ok_or_else(not_plain)?;
    let file = File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || !same_file(&looked, &metadata) {
        return Err(not_plain());
    }
    Ok((file, metadata))
}

/// Whether `metadata`'s file has been left long enough that no writer is at work on it.
pub(super) fn left_behind(metadata: &Metadata, now: SystemTime) -> bool {
    metadata
        .modified()
        .ok()
        .and_then(|modified| now.duration_since(modified).ok())
        .is_some_and(|age| age >= LEFT_BEHIND)
}

pub(super) fn seconds_of(time: SystemTime) -> Option<u64> {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .map(|since| since.as_secs())
}

/// Removes a copy: the `.meta` first, so that a failure halfway leaves a text nothing offers,
/// never an offer with nothing behind it. What has gone already is no failure.
pub(in crate::emergency) fn remove_copy(text: &Path, meta: Option<&Path>) -> io::Result<()> {
    let gone = |path: &Path| match fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    };
    if let Some(meta) = meta {
        gone(meta)?;
    }
    gone(text)
}
