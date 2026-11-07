//! File metadata on systems where it is decided later, which today means Windows
//! (ADR 0065, ADR 0067).
//!
//! Must not depend on: `control`, `privacy`, `process` or `text`.

use std::fs::File;
use std::io;
use std::path::Path;

use super::{AccessEntry, AccessList, Quarantine};

#[expect(
    clippy::unnecessary_wraps,
    reason = "the same signature as on macOS, where it can fail"
)]
#[cfg_attr(not(unix), allow(dead_code, reason = "used only by the Unix backends"))]
pub(super) fn list_at(_path: &Path) -> io::Result<Vec<AccessEntry>> {
    Ok(Vec::new())
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "the same signature as on macOS, where it can fail"
)]
pub(super) fn seal(_file: &File) -> io::Result<()> {
    Ok(())
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "the same signature as on macOS, where it can fail"
)]
pub(super) fn access_list_at(_path: &Path) -> io::Result<AccessList> {
    Ok(none())
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "the same signature as on macOS, where it can fail"
)]
pub(super) fn take_access_list(_file: &File) -> io::Result<AccessList> {
    Ok(none())
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "the same signature as on macOS, where it can fail"
)]
pub(super) fn give_access_list(_list: &AccessList, _file: &File) -> io::Result<()> {
    Ok(())
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "the same signature as on macOS, where it can fail"
)]
pub(super) fn clear_access_list_at(_path: &Path) -> io::Result<()> {
    Ok(())
}

pub(super) fn create_private(path: &Path) -> io::Result<File> {
    super::create_new_private(path)
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "the same signature as on macOS, where it can fail"
)]
pub(super) fn quarantine_at(_path: &Path) -> io::Result<Option<Quarantine>> {
    Ok(None)
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "the same signature as on macOS, where it can fail"
)]
pub(super) fn give_quarantine(_flag: &Quarantine, _file: &File) -> io::Result<()> {
    Ok(())
}

fn none() -> AccessList {
    AccessList {
        bytes: Vec::new(),
        denies_deleting: false,
    }
}
