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
pub(crate) mod ops;
mod order;
mod read;
pub(crate) mod rows;

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;

use oso_core::buffer::BufferId;
use oso_core::load::LoadSnapshot;
use oso_core::syntax::Highlights;

pub(crate) use read::list;

/// The most entries a listing keeps; past them it says it is cut (ADR 0134).
pub(crate) const MAX_ENTRIES: usize = 100_000;

/// How a listing orders its rows, directories first in each: oso-core's, as its record keeps it.
pub(crate) use oso_core::editor::ListingOrder as Sort;

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
    /// Whether to send partial listings while a large directory is read: not when the buffer
    /// shows a whole listing of it already, which a partial one would cut short.
    pub(crate) partials: bool,
    /// The names marked, which the rows show marked.
    pub(crate) marked: Arc<HashSet<OsString>>,
    /// Whether to mark every other row in their place, as `t` does, once the whole listing is
    /// read: the names marked then come back with it.
    pub(crate) invert: bool,
}

/// A row of a listing: what its line names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Row {
    /// Its name in the directory, `.` and `..` among them.
    pub(crate) name: OsString,
    /// Whether `RET` on it lists it: a directory, or a link a look found leads to one.
    pub(crate) enters: bool,
    /// When it was last modified, by which a listing by date is ordered.
    pub(crate) modified: Option<std::time::SystemTime>,
    /// The face of its name while it is not marked.
    pub(crate) face: Option<oso_core::syntax::Highlight>,
    /// The bytes its name takes in the text, as it is shown.
    pub(crate) name_len: usize,
}

/// The index in `rows`, a listing's ordered by `sort`, of the row named as `row` is, found by
/// what it is ordered by in O(log n), or by its name alone if that has changed. `.` and `..`
/// come first and are found by name.
pub(crate) fn position(rows: &[Row], sort: Sort, row: &Row) -> Option<usize> {
    let dots = rows
        .iter()
        .take(2)
        .take_while(|shown| shown.name == "." || shown.name == "..")
        .count();
    if let Some(index) = rows[..dots].iter().position(|shown| shown.name == row.name) {
        return Some(index);
    }
    let rest = &rows[dots..];
    if let Ok(index) = rest.binary_search_by(|shown| order::compare(shown, row, sort))
        && rest[index].name == row.name
    {
        return Some(dots + index);
    }
    position_of_name(rows, &row.name)
}

/// The index of the row named `name`, looked for one by one: for a name whose kind or time is
/// not known, as the one a command asks point to go to.
pub(crate) fn position_of_name(rows: &[Row], name: &std::ffi::OsStr) -> Option<usize> {
    rows.iter().position(|shown| shown.name == name)
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
    /// How many of its rows are marked.
    pub(crate) marked: usize,
    /// The directory's device and inode, read from the descriptor it was listed from, which an
    /// operation checks the directory it acts in against.
    pub(crate) identity: Option<(u64, u64)>,
    /// The names marked in place of the request's, when it asked to invert them.
    pub(crate) marked_names: Option<Arc<HashSet<OsString>>>,
    /// What the directory's metadata said just before it was read, which the lister sets, so
    /// that a change the watcher sees later is one since the listing.
    pub(crate) stamp: Option<oso_core::buffer::FileStamp>,
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
