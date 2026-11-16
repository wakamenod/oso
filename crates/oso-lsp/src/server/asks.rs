//! What the UI asks a server about a document, and what comes back (ADR 0138, Go to definition
//! and back, step 8): the definitions or references at a byte offset, and the completions there
//! (step 9). Step 10 adds its kind here.
//!
//! Must not depend on: the session's loop or the pipes; these are the values that cross
//! between the UI and the session thread.
//!
//! - One ask of each kind for each document is in flight; a newer one cancels the older with
//!   `$/cancelRequest`, and the older gets no answer.
//! - Each waits at most its kind's wait ([`AskKind::wait`]); a run that ends fails what waits.
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
    /// `textDocument/completion`, as the person asked or typed (step 9).
    Completion,
    /// `completionItem/resolve` of an item of the last completions (step 9b).
    Resolve,
}

impl AskKind {
    /// The request's method.
    pub fn method(self) -> &'static str {
        match self {
            Self::Definition => "textDocument/definition",
            Self::References => "textDocument/references",
            Self::Completion => "textDocument/completion",
            Self::Resolve => "completionItem/resolve",
        }
    }

    /// How long its answer is waited for (ADR 0138's table).
    pub fn wait(self) -> std::time::Duration {
        match self {
            Self::Definition | Self::References => crate::limits::DEFINITION_WAIT,
            Self::Completion | Self::Resolve => crate::limits::COMPLETION_WAIT,
        }
    }

    /// The word for it in a message: `definition failed`.
    pub fn noun(self) -> &'static str {
        match self {
            Self::Definition => "definition",
            Self::References => "references",
            Self::Completion => "completion",
            Self::Resolve => "resolve",
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
    pub context: AskContext,
}

/// What more an ask says (step 9b): nothing, for definitions and references; why completions
/// are asked for; or, for a resolve, which item of which answer, by the completions' ask's
/// serial and the item's place in its answer. A kind with the wrong context fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AskContext {
    #[default]
    None,
    Completion(crate::protocol::Invoked),
    Item {
        answer: u64,
        index: usize,
    },
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

/// What a server answered, by what was asked. Steps 9b and 10 add theirs.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Answered {
    /// Definitions or references: at most [`LOCATIONS_MAX`](crate::limits::LOCATIONS_MAX)
    /// places, in the server's order.
    Places(Vec<Location>),
    /// Completions (step 9).
    Completion(Completions),
    /// A resolved item's edits besides its own (step 9b).
    Resolved(Resolved),
}

/// What an item's `completionItem/resolve` gave: the item, by its answer's serial and its place
/// in it, and its edits besides its own, their ranges in bytes of the caller's text of
/// `revision`. `Debug` gives the counts only.
#[derive(Clone, PartialEq, Eq)]
pub struct Resolved {
    pub answer: u64,
    pub index: usize,
    pub revision: u64,
    pub extra: Vec<(Range<usize>, String)>,
}

impl std::fmt::Debug for Resolved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Resolved")
            .field("index", &self.index)
            .field("extra", &self.extra.len())
            .finish_non_exhaustive()
    }
}

/// The completions an answer offers, at most
/// [`COMPLETION_ITEMS_MAX`](crate::limits::COMPLETION_ITEMS_MAX), in the server's order; whether
/// it said the list is incomplete; and the caller's revision of the text their ranges are in.
/// `Debug` gives the counts only: an item can quote the person's text.
#[derive(Clone, PartialEq, Eq)]
pub struct Completions {
    pub revision: u64,
    pub items: Vec<Completion>,
    pub incomplete: bool,
}

impl std::fmt::Debug for Completions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Completions")
            .field("revision", &self.revision)
            .field("items", &self.items.len())
            .field("incomplete", &self.incomplete)
            .finish()
    }
}

/// One completion: its label, kind (1 to 25) and detail as the popup shows them, what the typed
/// word is matched against, the text it inserts, plain, and the byte range of the text of
/// [`Completions::revision`] that the text replaces, if the server gave one.
#[derive(Clone, PartialEq, Eq)]
pub struct Completion {
    pub label: String,
    pub kind: Option<u8>,
    pub detail: String,
    pub filter: Option<String>,
    pub text: String,
    pub range: Option<Range<usize>>,
    /// Its edits besides its own, if the answer gave them, in bytes of the same text (9b).
    pub extra: Vec<(Range<usize>, String)>,
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
