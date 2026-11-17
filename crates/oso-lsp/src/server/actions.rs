//! What a server's code actions come to for the UI (ADR 0138, step 13): each action of an answer,
//! as a list shows it and as it applies, and its edit placed in bytes of the texts the server
//! has of the documents open with it.
//!
//! Must not depend on: the session's loop or the pipes; these are the values that cross
//! between the UI and the session thread.
//!
//! - An edit of a document open with the server is placed in bytes of the caller's revision of
//!   the text the server has, which the UI applies only if its buffer is still at it (Q8).
//! - An edit of a file no document open with it holds is kept as the server counts it, with
//!   the encoding, for the UI to open and place (13b).
//! - `Debug` gives counts and titles only: an edit's text may quote what the server read.

use std::ops::Range;
use std::path::PathBuf;

use crate::position::Encoding;
use crate::protocol::Extra;

/// The code actions of an answer, in its order, at most
/// [`CODE_ACTIONS_MAX`](crate::limits::CODE_ACTIONS_MAX).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeActions {
    pub actions: Vec<CodeAction>,
}

/// One code action: its title and kind, cut, as a list shows them; whether the server prefers
/// it; why it cannot be applied, if the server says; its edit, or why it cannot be applied;
/// the command it runs, by name, if any (13b); and whether a resolve gives it its edit.
#[derive(Clone, PartialEq, Eq)]
pub struct CodeAction {
    pub title: String,
    pub kind: Option<String>,
    pub preferred: bool,
    pub disabled: Option<String>,
    pub edit: Option<Result<ServerEdit, String>>,
    pub command: Option<String>,
    pub resolves: bool,
}

impl std::fmt::Debug for CodeAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodeAction")
            .field("title", &self.title)
            .field("kind", &self.kind)
            .field("preferred", &self.preferred)
            .field("disabled", &self.disabled.is_some())
            .field("edit", &self.edit)
            .field("command", &self.command)
            .field("resolves", &self.resolves)
            .finish()
    }
}

/// A server's edit, by file: those of documents open with it, placed, and those of other files,
/// as the server counts them.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ServerEdit {
    pub open: Vec<OpenEdit>,
    pub unopened: Vec<UnopenedEdit>,
}

/// The edits of document `document`, in bytes of the caller's revision `revision`, in the
/// server's order.
#[derive(Clone, PartialEq, Eq)]
pub struct OpenEdit {
    pub document: u64,
    pub revision: u64,
    pub edits: Vec<(Range<usize>, String)>,
}

/// The edits of the file at `path`, which no document open with the server holds, as the
/// server counts them in `encoding`.
#[derive(Clone, PartialEq, Eq)]
pub struct UnopenedEdit {
    pub path: PathBuf,
    pub edits: Vec<Extra>,
    pub encoding: Encoding,
}

impl std::fmt::Debug for ServerEdit {
    /// The counts only: an edit's text may quote what the server read.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let edits = |open: usize, unopened: usize| open + unopened;
        f.debug_struct("ServerEdit")
            .field("open", &self.open.len())
            .field("unopened", &self.unopened.len())
            .field(
                "edits",
                &edits(
                    self.open.iter().map(|file| file.edits.len()).sum(),
                    self.unopened.iter().map(|file| file.edits.len()).sum(),
                ),
            )
            .finish()
    }
}

/// What a code action's `codeAction/resolve` gave: the action, by its answer's serial and its
/// place in it, and its edit, or why it cannot be applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionResolved {
    pub answer: u64,
    pub index: usize,
    pub edit: Result<ServerEdit, String>,
}
