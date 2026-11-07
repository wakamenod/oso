//! File metadata on systems with nothing to do: on Linux what a POSIX list grants shows in the
//! mode, and other systems are decided later (ADR 0065).
//!
//! Must not depend on: `control`, `privacy`, `process` or `text`.

use std::fs::File;
use std::io;
use std::path::Path;

use super::{AccessEntry, Sealed};

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
pub(super) fn seal(_file: &File) -> io::Result<Sealed> {
    Ok(Sealed { had_grant: false })
}
