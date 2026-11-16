//! What the UI asks a server about a document, and what comes back (ADR 0138, Go to definition
//! and back, step 8): the definitions or references at a byte offset. Steps 9 and 10 add their
//! kinds here.
//!
//! Must not depend on: the session's loop or the pipes; these are the values that cross
//! between the UI and the session thread.
//!
//! - One ask of each kind for each document is in flight; a newer one cancels the older with
//!   `$/cancelRequest`, and the older gets no answer.
//! - Each waits at most [`DEFINITION_WAIT`](crate::limits::DEFINITION_WAIT); a run that ends
//!   fails what waits.
//! - An answer wakes the UI at once.

use std::ops::Range;
use std::path::PathBuf;

use crate::position::{Encoding, Position};

/// What is asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AskKind {
    /// `textDocument/definition`.
    Definition,
    /// `textDocument/references`, the declaration included.
    References,
}

impl AskKind {
    /// The request's method.
    pub fn method(self) -> &'static str {
        match self {
            Self::Definition => "textDocument/definition",
            Self::References => "textDocument/references",
        }
    }

    /// The word for it in a message: `definition failed`.
    pub fn noun(self) -> &'static str {
        match self {
            Self::Definition => "definition",
            Self::References => "references",
        }
    }
}

/// An ask about the open document `document` at the byte offset `offset` of the caller's
/// revision `revision`, which must be the text last handed over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ask {
    pub document: u64,
    pub kind: AskKind,
    pub revision: u64,
    pub offset: usize,
}

/// The answer to an [`Ask`]: the places, at most [`LOCATIONS_MAX`](crate::limits::LOCATIONS_MAX),
/// in the server's order, or why there are none, for a person. A server's own message in it
/// is the server's text: shown, never logged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub document: u64,
    pub kind: AskKind,
    pub revision: u64,
    pub outcome: Result<Vec<Location>, String>,
}

/// A place an answer names: a file, by the path its `file:` URI reads as, and where in it.
#[derive(Clone, PartialEq, Eq)]
pub struct Location {
    pub path: PathBuf,
    pub target: Target,
}

impl std::fmt::Debug for Location {
    /// The range only: a path can name what a person keeps private.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Location")
            .field("target", &self.target)
            .finish_non_exhaustive()
    }
}

/// Where in its file a [`Location`] is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// In a document open with the server: byte offsets in the text of the caller's revision
    /// `revision`, which the caller maps through its edits since.
    Open {
        document: u64,
        revision: u64,
        range: Range<usize>,
    },
    /// In a file the server does not have open: a line and column as the server counts them,
    /// for the caller to place once it has the text.
    Lines {
        start: Position,
        end: Position,
        encoding: Encoding,
    },
}
