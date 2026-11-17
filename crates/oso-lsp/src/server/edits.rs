//! A server's workspace edit placed for the UI (ADR 0138, step 13): a code action's now, and a
//! server's own `workspace/applyEdit` (13b) and a rename's (step 14) later.
//!
//! Must not depend on: the session's loop or the pipes; these are the values that cross
//! between the UI and the session thread.
//!
//! - An edit of a document whose text the server has is placed in bytes of the caller's revision
//!   of that text, as it was when the request went out; the UI applies it only if its buffer is
//!   still at it (Q8).
//! - An edit of any other file is kept as the server counts it, with the encoding: a document
//!   kept from the server is one of them, which the UI must find by its path before it opens
//!   anything (13b).
//! - `Debug` gives counts only: an edit's text may quote what the server read.

use std::ops::Range;
use std::path::PathBuf;

use crate::position::Encoding;
use crate::protocol::Extra;

/// A server's edit, by file: those of documents whose text the server has, placed, and those
/// of other files, as the server counts them.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ServerEdit {
    pub open: Vec<OpenEdit>,
    pub unsent: Vec<UnsentEdit>,
}

/// The edits of document `document`, in bytes of the caller's revision `revision`, in the
/// server's order.
#[derive(Clone, PartialEq, Eq)]
pub struct OpenEdit {
    pub document: u64,
    pub revision: u64,
    pub edits: Vec<(Range<usize>, String)>,
}

/// The edits of the file at `path`, whose text the server does not have from the caller, as the
/// server counts them in `encoding`: no document open with it is there, or one kept from it.
#[derive(Clone, PartialEq, Eq)]
pub struct UnsentEdit {
    pub path: PathBuf,
    pub edits: Vec<Extra>,
    pub encoding: Encoding,
}

impl std::fmt::Debug for ServerEdit {
    /// The counts only: an edit's text may quote what the server read.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let edits = |open: usize, unsent: usize| open + unsent;
        f.debug_struct("ServerEdit")
            .field("open", &self.open.len())
            .field("unsent", &self.unsent.len())
            .field(
                "edits",
                &edits(
                    self.open.iter().map(|file| file.edits.len()).sum(),
                    self.unsent.iter().map(|file| file.edits.len()).sum(),
                ),
            )
            .finish()
    }
}
