//! The checks a file must pass before anything opens it, and removing a copy (ADR 0064).
//!
//! Must not depend on: `app`, `dispatch` or winit; these are plain functions of paths.

use std::fs::{self, File, Metadata};
use std::io;
use std::path::Path;
use std::time::SystemTime;

use oso_platform::privacy::{has_one_name, is_this_users};

use super::LEFT_BEHIND;
use crate::loader::same_file;

/// A regular file of this user's with one name, by what `symlink_metadata` says, before
/// anything opens it, as opening a pipe blocks. A hard link is a name for some other file's text.
pub(super) fn plain_own_file(path: &Path) -> Option<Metadata> {
    let metadata = fs::symlink_metadata(path).ok()?;
    let plain = metadata.is_file() && is_this_users(&metadata) && has_one_name(&metadata);
    plain.then_some(metadata)
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
    let still_plain = metadata.is_file() && is_this_users(&metadata) && has_one_name(&metadata);
    if !still_plain || !same_file(&looked, &metadata) {
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
