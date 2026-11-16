//! The search of Claude Code's past conversations (ADR 0145), in the prompt a project's search
//! uses: its input, its streaming rows, their highlights and the preview are the same; what is
//! searched, the scope, the rows' labels and what `RET` gives are this module's.
//!
//! Must not depend on: `command` or `keymap`, or the host's threads or I/O; the host reads the
//! transcripts and hands over the rows it found.
//!
//! - A scope is the label it shows and the roots and sessions a conversation must be in, or
//!   none for every conversation; `C-c C-s` moves to the next scope and searches again.
//! - A row is a conversation's label, a space, and the line of a message's first match. Its
//!   place keeps the session's id and the message's index, which `RET` and the preview give
//!   the host.
//! - Nothing of the input, the scope's roots or the rows reaches a log; `Debug` gives a
//!   generation at most.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use super::{Place, ProjectSearchRequest, To};
use crate::complete::{Candidate, WORKER_PAST};
use crate::editor::{CompletionStyle, Editor, WalkEnd};
use crate::minibuffer::{PromptError, PromptId};
use crate::project::{LineQuery, MAX_ROWS};

/// The search the host should start: of a project's files, or of conversations.
pub(super) enum Request {
    Files(ProjectSearchRequest),
    Conversations(ConversationSearchRequest),
}

/// Which conversations a search looks through (ADR 0145), and what its label calls them.
/// `Debug` gives whether it holds every conversation, never its roots.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ConversationScope {
    /// What the label names: `all`, `space NAME`, `project NAME`.
    pub label: String,
    /// What a conversation must be in, or none for every conversation.
    within: Option<Arc<Within>>,
}

/// What a conversation must be in to be in a scope: one of the directories it ran in is at or
/// under one of `roots`, component by component, or its session's id is one of `sessions`.
#[derive(Clone, Default, PartialEq, Eq)]
struct Within {
    roots: Vec<PathBuf>,
    sessions: Vec<Box<str>>,
}

impl fmt::Debug for ConversationScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConversationScope")
            .field("all", &self.within.is_none())
            .finish_non_exhaustive()
    }
}

impl ConversationScope {
    /// Every conversation on this machine.
    pub fn all() -> Self {
        Self {
            label: "all".to_owned(),
            within: None,
        }
    }

    /// The conversations of `sessions`, and those that ran at or under one of `roots`.
    pub fn within(label: String, roots: Vec<PathBuf>, sessions: Vec<Box<str>>) -> Self {
        Self {
            label,
            within: Some(Arc::new(Within { roots, sessions })),
        }
    }

    /// Whether the conversation of the session `id`, which ran in `directories`, is in it:
    /// `id` is one of its sessions, or a directory is at or under one of its roots, component
    /// by component, so `/w/ab` is not under `/w/a`.
    pub fn holds(&self, id: &str, directories: &[PathBuf]) -> bool {
        let Some(within) = &self.within else {
            return true;
        };
        within.sessions.iter().any(|session| **session == *id)
            || directories
                .iter()
                .any(|directory| within.roots.iter().any(|root| directory.starts_with(root)))
    }
}

/// A search of conversations the host should start (ADR 0145). `Debug` gives its generation
/// only.
#[derive(Clone)]
pub struct ConversationSearchRequest {
    /// The search it is, which [`Editor::conversation_rows_found`] is given back.
    pub generation: u64,
    pub scope: ConversationScope,
    /// What a message must match, all its components in it.
    pub query: Arc<LineQuery>,
    /// The live search's generation: the host stops at its next conversation once this has
    /// moved past [`ConversationSearchRequest::generation`].
    pub live: Arc<AtomicU64>,
}

impl fmt::Debug for ConversationSearchRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConversationSearchRequest")
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

/// A message a search of conversations found: its session's id, the label its row starts
/// with (its mark, name, place and age, and `›` or `‹`), the message's index in what the host
/// holds of the conversation, the line of its first match in the message, from 0, the column
/// of that match in characters from the line's start, and the line's text around it. `Debug`
/// says nothing of it.
#[derive(Clone)]
pub struct ConversationRow {
    pub session: Box<str>,
    pub label: Box<str>,
    pub message: usize,
    pub line: usize,
    pub column: usize,
    pub text: Box<str>,
}

impl fmt::Debug for ConversationRow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConversationRow").finish_non_exhaustive()
    }
}

/// The conversation's row `RET` took: its session's id and its message's index.
#[derive(Clone, PartialEq, Eq)]
pub struct ConversationHit {
    pub session: Box<str>,
    pub message: usize,
}

impl fmt::Debug for ConversationHit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConversationHit").finish_non_exhaustive()
    }
}

/// A conversation's text the host should give the preview of a row (ADR 0145): the messages
/// around `message` of the conversation of `session`, wrapped at `columns`, as an
/// [`Excerpt`](super::Excerpt) whose first line is 1. `Debug` gives its generation only.
#[derive(Clone)]
pub struct ConversationPreviewRequest {
    /// The read it is, which [`Editor::preview_read`] is given back.
    pub generation: u64,
    pub session: Box<str>,
    pub message: usize,
    /// The line of the row's match in the message, from 0, and its column, in characters from
    /// that line's start.
    pub line: usize,
    pub column: usize,
    /// The rows and columns the preview shows.
    pub rows: usize,
    pub columns: usize,
    /// The newest read's generation: stale once this has moved past
    /// [`ConversationPreviewRequest::generation`].
    pub live: Arc<AtomicU64>,
}

impl fmt::Debug for ConversationPreviewRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConversationPreviewRequest")
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

/// What a search of conversations keeps: the scopes it moves through, and the one it is in.
pub(super) struct Conversations {
    scopes: Vec<ConversationScope>,
    at: usize,
    /// The scope of every conversation, should `at` be past the scopes.
    all: ConversationScope,
}

impl Conversations {
    pub(super) fn scope(&self) -> &ConversationScope {
        self.scopes.get(self.at).unwrap_or(&self.all)
    }
}

/// The label of a search of conversations in `scope`.
fn label(scope: &ConversationScope) -> String {
    format!("Search conversations ({}):", scope.label)
}

impl Editor {
    /// Opens a prompt that searches Claude Code's past conversations (ADR 0145) in `scopes`,
    /// which `C-c C-s` moves through, starting at `at`, its input kept in the history
    /// `history`. It opens empty; each input that changes asks the host for a search
    /// ([`Editor::take_conversation_search_request`]), whose rows come to
    /// [`Editor::conversation_rows_found`].
    ///
    /// # Errors
    ///
    /// [`PromptError::TooDeep`] with as many prompts open as may be, and
    /// [`PromptError::Invalid`] with no scope, or while a search is open already. Nothing
    /// changes.
    pub fn open_conversation_search(
        &mut self,
        scopes: Vec<ConversationScope>,
        at: usize,
        history: &'static str,
    ) -> Result<PromptId, PromptError> {
        if scopes.is_empty() || self.project_search.prompt.is_some() {
            return Err(PromptError::Invalid);
        }
        let conversations = Conversations {
            at: at.min(scopes.len() - 1),
            scopes,
            all: ConversationScope::all(),
        };
        let style = CompletionStyle {
            given_order: true,
            preview: true,
            ..CompletionStyle::default()
        };
        let id =
            self.open_completion_as(label(conversations.scope()), history, Arc::from([]), style)?;
        self.start_search_prompt(id, PathBuf::new(), Some(conversations));
        Ok(id)
    }

    /// Whether the innermost prompt searches conversations.
    fn searches_conversations(&self) -> bool {
        self.searches_project() && self.project_search.conversations.is_some()
    }

    /// The search of conversations the host should start, once.
    pub fn take_conversation_search_request(&mut self) -> Option<ConversationSearchRequest> {
        match self.project_search.request.take() {
            Some(Request::Conversations(request)) => Some(request),
            other => {
                self.project_search.request = other;
                None
            }
        }
    }

    /// Moves the innermost prompt's search of conversations to its next scope, and searches
    /// again with the same input. Returns whether it searches conversations.
    pub fn next_conversation_scope(&mut self) -> bool {
        if !self.searches_conversations() {
            return false;
        }
        let Some(conversations) = self.project_search.conversations.as_mut() else {
            return false;
        };
        conversations.at = (conversations.at + 1) % conversations.scopes.len().max(1);
        let label = label(conversations.scope());
        if let Some(prompt) = self.prompts.last_mut() {
            prompt.label = label;
        }
        self.project_search.input = None;
        self.project_search.last_whole = None;
        self.search_input_changed();
        self.relayout_prompt();
        true
    }

    /// Takes in rows the search of conversations `generation` found, and with `end`, that it
    /// ended, as [`Editor::rows_found`] does for a project's files.
    pub fn conversation_rows_found(
        &mut self,
        generation: u64,
        rows: Vec<ConversationRow>,
        end: Option<WalkEnd>,
    ) {
        let search = &mut self.project_search;
        let Some(room) = search
            .prompt
            .filter(|_| generation == search.generation && search.running)
            .filter(|_| search.conversations.is_some())
            .map(|_| MAX_ROWS.saturating_sub(search.places.len()))
        else {
            if rows.len() > WORKER_PAST {
                self.free_on_the_worker(Box::new(rows));
            }
            return;
        };
        let cut_here = rows.len() > room;
        let mut added = Vec::with_capacity(rows.len().min(room));
        for row in rows.into_iter().take(room) {
            let prefix = row.label.len() + 1;
            search.places.push(Place {
                line: row.line,
                column: row.column,
                prefix,
                to: To::Message {
                    session: row.session,
                    message: row.message,
                },
            });
            added.push(Candidate::unmatched(
                format!("{} {}", row.label, row.text),
                "",
            ));
        }
        self.add_search_rows(added, end, cut_here);
    }

    /// Keeps the scope of the space in front, which a search of conversations offers after
    /// every conversation's, or none while no space is in front. The host gives it whenever
    /// the space in front or what it holds changes.
    pub fn set_space_scope(&mut self, scope: Option<ConversationScope>) {
        self.project_search.space_scope = scope;
    }

    /// The scopes a search of conversations moves through: every conversation, the space in
    /// front's if the host gave one, and the selected buffer's project's, with its main
    /// repository's root for a worktree checked to be one, if it is in a project.
    pub fn conversation_scopes(&self) -> Vec<ConversationScope> {
        let mut scopes = vec![ConversationScope::all()];
        scopes.extend(self.project_search.space_scope.clone());
        let (_, buffer) = self.current();
        if let Some(project) = buffer.project().and_then(|id| self.project(id)) {
            let mut roots = vec![project.root().to_path_buf()];
            if let Some(main) = &project.found().checked_main
                && main.path != project.root()
            {
                roots.push(main.path.clone());
            }
            let label = format!("project {}", project.name());
            scopes.push(ConversationScope::within(label, roots, Vec::new()));
        }
        scopes
    }

    /// The conversation's row the last search's `RET` took, once.
    pub fn take_conversation_hit(&mut self) -> Option<ConversationHit> {
        self.project_search.conversation_hit.take()
    }
}

#[cfg(test)]
#[path = "conversations_tests.rs"]
mod tests;
