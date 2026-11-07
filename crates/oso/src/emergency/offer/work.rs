//! What an offer asks the host to start, and what those threads report (ADR 0064).
//!
//! Must not depend on: `app`, `dispatch` or winit; these are values and what runs off the UI
//! thread, and the host owns the threads.

use std::path::PathBuf;

use oso_core::buffer::{Buffer, BufferId, Verdict};
use oso_core::display_map::TabWidth;
use oso_core::load::{LoadSnapshot, decode_all};

use crate::emergency::compare::{NoDiff, diff_files};
use crate::emergency::copies::{Scan, ScanRequest, open_checked, remove_copy};

/// What the host must start, each on a thread of its own, for the offer to go on.
pub enum CopyWork {
    /// Look through the directory again.
    Scan(ScanRequest),
    /// Fill an internal buffer with a copy's text, no further than `limit`.
    Load {
        buffer: BufferId,
        path: PathBuf,
        limit: u64,
    },
    /// Show how a copy differs from its file.
    Compare(Comparing),
    /// Remove a copy.
    Delete(Deleting),
    /// Free what a buffer holds, off the UI thread.
    Free(Box<Buffer>),
}

/// A compare to run.
pub struct Comparing {
    pub(super) key: PathBuf,
    pub(super) name: String,
    pub(super) file: PathBuf,
    pub(super) verdict: (Vec<PathBuf>, Verdict),
}

/// A removal to run.
pub struct Deleting {
    pub(super) text: PathBuf,
    pub(super) meta: Option<PathBuf>,
    /// What the message calls it.
    pub(super) name: String,
}

/// What a thread of the offer reports. A `Debug` of it shows no text.
pub enum CopiesEvent {
    Scan(Box<Scan>),
    Compared(Box<Compared>),
    Deleted {
        name: String,
        problem: Option<String>,
    },
}

impl std::fmt::Debug for CopiesEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Scan(_) => "Scan",
            Self::Compared(_) => "Compared",
            Self::Deleted { .. } => "Deleted",
        })
    }
}

/// How a compare came out.
pub struct Compared {
    pub(super) key: PathBuf,
    pub(super) name: String,
    pub(super) verdict: (Vec<PathBuf>, Verdict),
    /// The diff as a buffer's content, or what is known instead.
    pub(super) result: Result<LoadSnapshot, String>,
}

impl Comparing {
    /// Reads both sides and diffs them. A panic costs only this compare.
    pub fn run(self) -> Compared {
        let Self {
            key,
            name,
            file,
            verdict,
        } = self;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
            || -> Result<LoadSnapshot, String> {
                let (copy, metadata) = open_checked(&key).map_err(|e| e.to_string())?;
                diff_files(&file, copy, metadata.len())
                    .map_err(|no_diff: NoDiff| no_diff.to_string())
                    .map(|diff| {
                        let diff = if diff.is_empty() {
                            "The file and the copy hold the same text.\n".to_owned()
                        } else {
                            diff
                        };
                        decode_all(diff.as_bytes(), TabWidth::default())
                    })
            },
        ))
        .unwrap_or_else(|_| Err("comparing them failed".to_owned()));
        Compared {
            key,
            name,
            verdict,
            result,
        }
    }
}

impl Deleting {
    /// Removes the files, and says how that went.
    pub fn run(self) -> CopiesEvent {
        let removed = remove_copy(&self.text, self.meta.as_deref());
        CopiesEvent::Deleted {
            name: self.name,
            problem: removed.err().map(|e| e.to_string()),
        }
    }
}
