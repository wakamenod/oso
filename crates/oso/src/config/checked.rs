//! Reading a file of the person's own with ADR 0108's checks: `config.toml`, the files beside
//! it, and a file the configuration names, such as migemo's dictionary (ADR 0116). Through
//! links, refused if other users can change it or the way to it, a regular file, the one
//! judged, within a size, in UTF-8.
//!
//! Must not depend on: `app` or `dispatch`; it reads one file and says what it found, and
//! parsing it is the caller's.

use std::io::{self, Read};
use std::path::Path;

use oso_platform::file_meta::open_for_reading;
use oso_platform::privacy::{self, Writers};

use super::watch::MAX_BYTES;
use crate::loader::same_file;

/// Why a file was not read.
pub(super) enum Unread {
    Missing,
    Refused(String),
}

/// The text of `file`, through links, if it passes the checks (ADR 0108, ADR 0110): a regular
/// file of at most [`MAX_BYTES`] of UTF-8 that only this user or root can change, nor anything
/// on the way to it, through every link. The way is judged before the file is opened, and the
/// file opened must be the one judged. With whether the users of a group can change it or the
/// way.
pub(super) fn read(file: &Path) -> Result<(String, bool), Unread> {
    read_within(file, MAX_BYTES)
}

/// The text of a file the person names in the configuration, with [`read`]'s checks and a
/// limit of `max` bytes: migemo's dictionary (ADR 0116). With whether the users of a group can
/// change it or the way; else why it was not read, which does not quote it.
pub fn read_checked(file: &Path, max: u64) -> Result<(String, bool), String> {
    read_within(file, max).map_err(|unread| match unread {
        Unread::Missing => "there is no such file".to_owned(),
        Unread::Refused(why) => why,
    })
}

/// [`read`], of at most `max` bytes.
fn read_within(file: &Path, max: u64) -> Result<(String, bool), Unread> {
    let refused = |why: &str| Err(Unread::Refused(why.to_owned()));
    let who = match privacy::who_can_change(file) {
        Ok(who) => who,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(Unread::Missing),
        Err(_) => return refused("the way to it cannot be looked at"),
    };
    if who.file == Writers::Anyone {
        return refused("other users can change it");
    }
    if who.way == Writers::Anyone {
        return refused("other users can change a directory or link on the way to it");
    }
    if !who.metadata.is_file() {
        return refused("it is not a regular file");
    }
    let too_large = || Err(Unread::Refused(format!("it is larger than {}", size(max))));
    if who.metadata.len() > max {
        return too_large();
    }
    let mut opened = match open_for_reading(file) {
        Ok(Some((opened, metadata))) if same_file(&metadata, &who.metadata) => opened,
        Ok(_) => return refused("it changed as it was read"),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(Unread::Missing),
        Err(e) => return Err(Unread::Refused(format!("it cannot be opened: {e}"))),
    };
    // Room for what the file holds, judged within `max`, and one byte more to see it grew.
    let room = usize::try_from(who.metadata.len())
        .unwrap_or(0)
        .saturating_add(1);
    let mut bytes = Vec::with_capacity(room);
    if let Err(e) = (&mut opened).take(max + 1).read_to_end(&mut bytes) {
        return Err(Unread::Refused(format!("it cannot be read: {e}")));
    }
    if bytes.len() as u64 > max {
        return too_large();
    }
    let Ok(text) = String::from_utf8(bytes) else {
        return refused("it is not UTF-8 text");
    };
    Ok((text, who.file.max(who.way) == Writers::Group))
}

/// `max` bytes, in KiB from one and in MiB from one.
fn size(max: u64) -> String {
    if max >= 1024 * 1024 {
        format!("{} MiB", max / (1024 * 1024))
    } else if max >= 1024 {
        format!("{} KiB", max / 1024)
    } else {
        format!("{max} bytes")
    }
}

#[cfg(test)]
mod tests;
