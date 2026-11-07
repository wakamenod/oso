//! What a save keeps of the file it replaces beyond its text and its access list: its owner, its group and
//! its mode, and what a new file gets where it is made (ADR 0059, ADR 0067).
//!
//! Must not depend on: `app`, winit or the GPU.

use std::fs::{File, Metadata, Permissions};
use std::io;
use std::path::Path;

use oso_core::buffer::FileStamp;
use oso_platform::file_meta::AccessList;

use super::{SaveError, Temporary};

/// The mode a program asks for a new file, which the umask then narrows. It means nothing on
/// Windows.
const DEFAULT_MODE: u32 = 0o666;

/// What a file made in a directory gets there, as any program would make it: the mode the umask
/// gives and the access list the directory gives.
pub(super) struct Made {
    pub(super) permissions: Permissions,
    pub(super) list: AccessList,
}

impl Made {
    /// Makes an empty file in `directory`, takes what it got, and removes it. Its list comes off
    /// first, so that it can be removed whatever the list says.
    pub(super) fn there(directory: &Path) -> Result<Self, SaveError> {
        let (_probe, file) = Temporary::create(directory, DEFAULT_MODE)?;
        let permissions = file.metadata()?.permissions();
        let list = AccessList::take_from(&file).map_err(SaveError::AccessList)?;
        Ok(Self { permissions, list })
    }
}

/// Whether the attributes the new file took are out of date by now, as the rename would
/// then undo a `chmod` made during the write. A file that came back counts, since it
/// was not there to take them from. One deleted meanwhile has nothing left to keep.
pub(super) fn attributes_moved(before: Option<&Metadata>, now: Option<&Metadata>) -> bool {
    match (before, now) {
        (Some(before), Some(now)) => attributes(before) != attributes(now),
        (None, Some(_)) => true,
        (_, None) => false,
    }
}

#[cfg(unix)]
fn attributes(metadata: &Metadata) -> (u32, u32, u32) {
    use std::os::unix::fs::MetadataExt;
    (metadata.mode(), metadata.uid(), metadata.gid())
}

#[cfg(not(unix))]
fn attributes(metadata: &Metadata) -> bool {
    metadata.permissions().readonly()
}

/// Gives `file` the owner and group of the file it replaces, if there is one, and says whether
/// the group could be kept.
#[cfg(unix)]
pub(super) fn keep_owner(file: &File, replaced: Option<&Metadata>) -> io::Result<bool> {
    use std::os::unix::fs::{MetadataExt, fchown};

    let Some(old) = replaced else {
        return Ok(true);
    };
    let new = file.metadata()?;
    if old.uid() != new.uid() {
        fchown(file, Some(old.uid()), None).map_err(|_| {
            io::Error::other("the file belongs to another user, and a save would take it over")
        })?;
    }
    Ok(old.gid() == new.gid() || fchown(file, None, Some(old.gid())).is_ok())
}

/// Gives `file` the mode of the file it replaces. If that file was deleted since it was `read`, it
/// gets its mode back, as if its group were lost, and a file that never existed gets what a file
/// `made` there gets. Extended attributes other than the quarantine flag, and other hard links, do
/// not carry over (ADR 0059, ADR 0067).
#[cfg(unix)]
pub(super) fn keep_mode(
    file: &File,
    replaced: Option<&Metadata>,
    read: Option<FileStamp>,
    made: Option<Permissions>,
    group_kept: bool,
) -> io::Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let Some(old) = replaced else {
        if let Some(read) = read {
            return file.set_permissions(Permissions::from_mode(kept_mode(read.mode, false)));
        }
        return made.map_or(Ok(()), |made| file.set_permissions(made));
    };
    file.set_permissions(Permissions::from_mode(kept_mode(old.mode(), group_kept)))
}

/// The mode the new file gets from the `mode` of the old one. Set-user-ID and set-group-ID
/// go, as they do when a file is written. If the file ends up in another group, that
/// group gets no more than everyone else, since the bits were meant for the old one.
#[cfg(unix)]
pub(super) fn kept_mode(mode: u32, group_kept: bool) -> u32 {
    let mode = mode & 0o777;
    if group_kept {
        mode
    } else {
        mode & !(0o070 & !((mode & 0o007) << 3))
    }
}

#[cfg(not(unix))]
#[expect(
    clippy::unnecessary_wraps,
    reason = "the same signature as on Unix, where it can fail"
)]
pub(super) fn keep_owner(_file: &File, _replaced: Option<&Metadata>) -> io::Result<bool> {
    Ok(true)
}

#[cfg(not(unix))]
pub(super) fn keep_mode(
    file: &File,
    replaced: Option<&Metadata>,
    _read: Option<FileStamp>,
    _made: Option<Permissions>,
    _group_kept: bool,
) -> io::Result<()> {
    match replaced {
        Some(old) => file.set_permissions(old.permissions()),
        None => Ok(()),
    }
}
