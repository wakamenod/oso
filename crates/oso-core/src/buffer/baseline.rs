//! What a buffer last knew of its file on disk (ADR 0029, ADR 0059). A save compares
//! this with what is there right before it replaces the file, so that a change made
//! by anything else is kept.
//!
//! Must not depend on: file I/O; the host reads these facts and hands them over.

use std::path::PathBuf;
use std::time::SystemTime;

/// A regular file, as it was read or written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileStamp {
    pub modified: SystemTime,
    pub len: u64,
    /// The Unix mode bits, or 0 elsewhere. A file deleted since gets them back.
    pub mode: u32,
}

impl FileStamp {
    /// Whether `other` is the same content by what can be told without reading it. The
    /// mode does not count: `chmod` changes no text.
    pub fn same_content(self, other: Self) -> bool {
        self.modified == other.modified && self.len == other.len
    }
}

/// Where a buffer's file is and what was there. A buffer has none until the host says,
/// and a save refuses without one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Baseline {
    /// The real place the path led: links followed, and the directory canonical. A save
    /// goes only here, so a link or a directory changed since cannot redirect the text.
    pub location: PathBuf,
    /// The file there, or `None` if there was none: one that appears later is a change.
    pub file: Option<FileStamp>,
}
