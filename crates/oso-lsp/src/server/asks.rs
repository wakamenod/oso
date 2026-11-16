//! What the UI asks a server about a document, and what comes back (ADR 0138, Go to definition
//! and back, step 8): the definitions or references at a byte offset, and the description of
//! what is there (Hover, step 10). Step 9 adds its kind here.
//!
//! Must not depend on: the session's loop or the pipes; these are the values that cross
//! between the UI and the session thread.
//!
//! - One ask of each kind for each document is in flight; a newer one cancels the older with
//!   `$/cancelRequest`, and the older gets no answer.
//! - Each waits at most [`AskKind::wait`]: a definition or references
//!   [`DEFINITION_WAIT`], a description [`COMPLETION_WAIT`]; a run that ends fails what waits.
//! - An answer wakes the UI at once.

use std::ops::Range;
use std::path::PathBuf;
use std::time::Duration;

use crate::limits::{COMPLETION_WAIT, DEFINITION_WAIT};
use crate::position::{Encoding, Position};
use crate::protocol::Hover;

/// What is asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AskKind {
    /// `textDocument/definition`.
    Definition,
    /// `textDocument/references`, the declaration included.
    References,
    /// `textDocument/hover` (step 10).
    Hover,
}

impl AskKind {
    /// The request's method.
    pub fn method(self) -> &'static str {
        match self {
            Self::Definition => "textDocument/definition",
            Self::References => "textDocument/references",
            Self::Hover => "textDocument/hover",
        }
    }

    /// How long its answer may take.
    pub fn wait(self) -> Duration {
        match self {
            Self::Definition | Self::References => DEFINITION_WAIT,
            Self::Hover => COMPLETION_WAIT,
        }
    }

    /// The word for it in a message: `definition failed`.
    pub fn noun(self) -> &'static str {
        match self {
            Self::Definition => "definition",
            Self::References => "references",
            Self::Hover => "description",
        }
    }
}

/// An ask about the open document `document` at the byte offset `offset` of the caller's
/// revision `revision`, which must be the text last handed over. `serial`, the caller's own,
/// comes back with the answer, so that an answer is told from one to an earlier ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ask {
    pub serial: u64,
    pub document: u64,
    pub kind: AskKind,
    pub revision: u64,
    pub offset: usize,
}

/// The answer to an [`Ask`], with its serial: what the server answered, or why there is
/// nothing, for a person. A server's own message in it is the server's text: shown, never
/// logged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub serial: u64,
    pub document: u64,
    pub kind: AskKind,
    pub revision: u64,
    pub outcome: Result<Answered, String>,
}

/// What a server answered, by what was asked. Steps 9 and 10 add theirs.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Answered {
    /// Definitions or references: at most [`LOCATIONS_MAX`](crate::limits::LOCATIONS_MAX)
    /// places, in the server's order.
    Places(Vec<Location>),
    /// A description: the hover's text, or `None` for one that describes nothing.
    Hover(Option<Hover>),
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
    /// `revision`, which the caller maps through its edits since; and the line and column as
    /// the server counts them, for a caller whose buffer is gone, or can no longer map them.
    Open {
        document: u64,
        revision: u64,
        range: Range<usize>,
        start: Position,
        encoding: Encoding,
    },
    /// In a file the server does not have open: a line and column as the server counts them,
    /// for the caller to place once it has the text.
    Lines {
        start: Position,
        end: Position,
        encoding: Encoding,
    },
}
