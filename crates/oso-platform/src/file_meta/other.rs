//! File metadata on systems with nothing to do yet: on Linux a list's effect shows in the mode,
//! and the rest are recorded as later (ADR 0065).
//!
//! Must not depend on: `control`, `privacy`, `process` or `text`.

use std::fs::File;
use std::io;
use std::path::Path;

use super::{ListEntry, Sealed};

#[expect(
    clippy::unnecessary_wraps,
    reason = "the same signature as on macOS, where it can fail"
)]
#[cfg_attr(not(unix), allow(dead_code, reason = "used only by the Unix backends"))]
pub(super) fn list_at(_path: &Path) -> io::Result<Vec<ListEntry>> {
    Ok(Vec::new())
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "the same signature as on macOS, where it can fail"
)]
pub(super) fn seal(_file: &File) -> io::Result<Sealed> {
    Ok(Sealed {
        grants_others: false,
    })
}
