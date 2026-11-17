//! What a server's code actions come to for the UI (ADR 0138, step 13): each action of an answer,
//! as a list shows it and as it applies, with its edit placed (`edits`), and what a resolve of
//! one gives.
//!
//! Must not depend on: the session's loop or the pipes; these are the values that cross
//! between the UI and the session thread.
//!
//! - A command an action runs is named; its arguments stay with the action the session keeps as
//!   the server gave it, which the UI names by its answer's serial and its place (13b).
//! - `Debug` gives counts and titles only: an edit's text may quote what the server read.

use super::ServerEdit;

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

/// What a code action's `codeAction/resolve` gave: the action, by its answer's serial and its
/// place in it, its edit, if it has one, or why it cannot be applied, and the command it runs,
/// by name, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionResolved {
    pub answer: u64,
    pub index: usize,
    pub edit: Option<Result<ServerEdit, String>>,
    pub command: Option<String>,
}
