//! Directory listings, as Emacs's dired shows them (ADR 0134): what a listing is asked for, the
//! entries read on the lister's threads, their order, and the text, highlights and row table
//! each listing builds there, so the UI thread does no O(n) work.
//!
//! Must not depend on: `app`, `dispatch`, `commands`, `lister` or winit; the lister runs
//! [`list`] on its threads and hands back what it builds.
//!
//! Nothing listed is opened or read: an entry's face and columns come from what listing it
//! said, never from its bytes. No process starts.

mod build;
mod order;
mod read;
pub(crate) mod rows;

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;

use oso_core::buffer::BufferId;
use oso_core::load::LoadSnapshot;
use oso_core::syntax::Highlights;

pub(crate) use read::list;

/// The most entries a listing keeps; past them it says it is cut (ADR 0134).
pub(crate) const MAX_ENTRIES: usize = 100_000;

/// How a listing orders its rows, directories first in each.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Sort {
    /// By name, ignoring ASCII case, then by bytes.
    #[default]
    Name,
    /// By modification time, newest first, then by name.
    Time,
}

impl Sort {
    /// What the modeline says of it, as dired's `Dired by name`.
    pub(crate) fn note(self) -> &'static str {
        match self {
            Self::Name => "by name",
            Self::Time => "by date",
        }
    }

    /// The other order, as `s` toggles them.
    pub(crate) fn toggled(self) -> Self {
        match self {
            Self::Name => Self::Time,
            Self::Time => Self::Name,
        }
    }
}

/// What a listing shows: its order and whether its rows show their details.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Options {
    pub(crate) sort: Sort,
    pub(crate) details: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            sort: Sort::Name,
            details: true,
        }
    }
}

/// A listing asked of the lister.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Request {
    /// The listing buffer it is for.
    pub(crate) buffer: BufferId,
    /// Which of that buffer's requests this is: only the newest is shown.
    pub(crate) generation: u64,
    pub(crate) directory: PathBuf,
    pub(crate) options: Options,
    /// The home directory, which the header writes as `~`.
    pub(crate) home: Option<PathBuf>,
}

/// A row of a listing: what its line names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Row {
    /// Its name in the directory, `.` and `..` among them.
    pub(crate) name: OsString,
    /// Whether `RET` on it lists it: a directory, or a link a look found leads to one.
    pub(crate) enters: bool,
}

/// A listing built on a lister's thread.
pub(crate) struct Built {
    pub(crate) text: LoadSnapshot,
    pub(crate) highlights: Highlights,
    /// What each row's line names, the first row on line [`Built::header_lines`].
    pub(crate) rows: Arc<[Row]>,
    pub(crate) header_lines: usize,
    /// The column of each row's name.
    pub(crate) name_column: usize,
    /// The directory's real place, and each link's target on the way, for its verdict; `None`
    /// if it could not be found.
    pub(crate) resolved: Option<Vec<PathBuf>>,
    /// Whether every entry is in; a listing still reading shows those read so far.
    pub(crate) done: bool,
}

impl std::fmt::Debug for Built {
    /// Counts only: a listing's names stay out of logs.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Built")
            .field("rows", &self.rows.len())
            .field("bytes", &self.text.text().len())
            .field("done", &self.done)
            .finish_non_exhaustive()
    }
}

/// What a lister's thread answers for a listing: one or more partial ones, then the whole one,
/// or why it could not be read.
#[derive(Debug)]
pub(crate) struct Answer {
    pub(crate) buffer: BufferId,
    pub(crate) generation: u64,
    pub(crate) built: Result<Built, String>,
}
