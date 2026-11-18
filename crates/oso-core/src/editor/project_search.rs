//! A project's search for `project-search` (ADR 0114): the prompt whose input is what to look
//! for, the search the host runs for each input, and the rows it sends, put after the rows shown
//! as they come. The same prompt searches past conversations (ADR 0145, `conversations.rs`) and
//! a buffer's lines (ADR 0154, `lines.rs`), each a [`Source`] of its own.
//!
//! Must not depend on: `command` or `keymap`, or the host's threads or I/O; the host searches
//! the files and hands over the rows it found.
//!
//! Each input that changes starts a search with a new generation, once a component that is not
//! excluded has two characters, or for a buffer's lines at once; the one before stops at its
//! next file or block. An input that
//! narrows that of a search that ended without a limit asks the host to look only through the
//! lines that search found. The input never filters the rows: each search's rows are the list,
//! in the order they come, and none moves once shown. The count shows `…` while it runs, and
//! `+` past a limit. Nothing of the input or the rows reaches a log; `Debug` gives the
//! generation only.

use std::fmt;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use super::file_preview::FilePreview;
use super::migemo::Cache;
use super::{CompletionStyle, Editor, WalkEnd};
use crate::buffer::BufferId;
use crate::complete::{Candidate, Candidates, MAX_INPUT_BYTES, WORKER_PAST};
use crate::minibuffer::{PromptError, PromptId};
use crate::project::{LineQuery, MAX_ROWS};
pub use conversations::{
    ConversationHit, ConversationPreviewRequest, ConversationRow, ConversationScope,
    ConversationSearchRequest,
};
use conversations::{Conversations, Request};
use lines::Lines;
pub use lines::{LineRow, LineSearchRequest, LinesEnd};
pub(in crate::editor) use preview::PreviewCellsKey;

mod conversations;
mod lines;
mod preview;

/// What a row of a file that may hold secrets says in place of its text (ADR 0114).
const WITHHELD: &str = "may hold secrets";

/// A search of a project's files the host should start, as `M-s r` asks (ADR 0114). `Debug`
/// gives its generation only.
#[derive(Clone)]
pub struct ProjectSearchRequest {
    /// The search it is, which [`Editor::rows_found`] is given back.
    pub generation: u64,
    /// The project's root, which the walk starts from and the rows' paths are relative to.
    pub root: PathBuf,
    /// What a line must match.
    pub query: Arc<LineQuery>,
    /// The live search's generation: the host stops at its next file or block once this has
    /// moved past [`ProjectSearchRequest::generation`].
    pub live: Arc<AtomicU64>,
    /// The search whose lines hold every one this one can find, as its input narrows that one's
    /// and it ended without a limit: the host looks only through them, if it still has them.
    pub narrow: Option<u64>,
}

impl fmt::Debug for ProjectSearchRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProjectSearchRequest")
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

/// A line a search found: its file's path from the root, its number from 1, the offset of its
/// start in the file in bytes, which the preview reads from, the column of its first match in
/// characters, and what of it a row shows, empty for a file that may hold secrets, whose text
/// is never shown. `Debug` says nothing of it.
#[derive(Clone)]
pub struct ProjectSearchRow {
    pub path: Box<str>,
    pub line: usize,
    pub offset: u64,
    pub column: usize,
    pub text: Box<str>,
    pub secret: bool,
}

impl fmt::Debug for ProjectSearchRow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProjectSearchRow").finish_non_exhaustive()
    }
}

/// The row `RET` took: the file, the line from 1 and the column in characters, from 0.
#[derive(Clone, PartialEq, Eq)]
pub struct ProjectSearchHit {
    pub path: PathBuf,
    pub line: usize,
    pub column: usize,
}

impl fmt::Debug for ProjectSearchHit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProjectSearchHit")
            .field("line", &self.line)
            .field("column", &self.column)
            .finish_non_exhaustive()
    }
}

/// What a row keeps beside its candidate: where it leads, the line and column of its match,
/// and how much of its text is its prefix, drawn in the annotation's face.
struct Place {
    /// The line, from 1 in a file and from 0 in a message.
    line: usize,
    column: usize,
    /// The bytes of `path:line: `, or the whole text for a file that may hold secrets, or a
    /// conversation's row's label and the space after it, or a buffer's line's padded number
    /// and `: `.
    prefix: usize,
    to: To,
}

/// Where a row leads.
enum To {
    /// A project's file: the bytes of its path at the start of the candidate's text, which
    /// starts with it, and the offset of the line in it, as the search found it.
    File {
        path: usize,
        offset: u64,
        secret: bool,
    },
    /// A conversation's message (ADR 0145): its session's id and the message's index in what
    /// the host holds.
    Message { session: Box<str>, message: usize },
    /// A line of the buffer a line search reads (ADR 0154): where its first match starts, in
    /// the revision the search read.
    Line { offset: usize },
}

impl Place {
    /// Whether the row is of a file that may hold secrets, whose text never shows.
    fn secret(&self) -> bool {
        matches!(self.to, To::File { secret: true, .. })
    }

    /// The bytes of the file's path at the start of the row's text; none for a message or a
    /// buffer's line.
    fn path(&self) -> usize {
        match self.to {
            To::File { path, .. } => path,
            To::Message { .. } | To::Line { .. } => 0,
        }
    }
}

/// What a search's prompt looks through: a project's files (ADR 0114), conversations (ADR
/// 0145) or a buffer's lines (ADR 0154).
#[derive(Default)]
enum Source {
    #[default]
    Files,
    Conversations(Conversations),
    Lines(Lines),
}

impl Source {
    fn conversations(&self) -> Option<&Conversations> {
        match self {
            Self::Conversations(conversations) => Some(conversations),
            _ => None,
        }
    }

    fn lines(&self) -> Option<&Lines> {
        match self {
            Self::Lines(lines) => Some(lines),
            _ => None,
        }
    }
}

/// What the prompt that searches a project keeps beside its rows.
#[derive(Default)]
pub(super) struct ProjectSearch {
    prompt: Option<PromptId>,
    /// The project's root, for a search of its files.
    root: PathBuf,
    source: Source,
    /// The generation of the search the rows are of.
    generation: u64,
    /// The newest search's generation, shared with the host, which stops once it moves on.
    live: Arc<AtomicU64>,
    /// The search to start, until the host takes it.
    request: Option<Request>,
    /// The scope of the space in front, as the host keeps it, which outlives each prompt.
    space_scope: Option<ConversationScope>,
    /// The search has not ended.
    running: bool,
    /// It stopped at a limit.
    cut: bool,
    /// The input searched for last, once one was: `None` for one too long to search for.
    input: Option<Option<String>>,
    query: Option<Arc<LineQuery>>,
    /// The last search that ended without a limit, which an input that narrows its own looks
    /// through again.
    last_whole: Option<(u64, Arc<LineQuery>)>,
    /// Each row's place, by its index in the list.
    places: Vec<Place>,
    /// The row `RET` took, for the command.
    hit: Option<ProjectSearchHit>,
    /// The conversation's row `RET` took, for the command.
    conversation_hit: Option<ConversationHit>,
    /// The buffer and the offset of the line's match `RET` took, for the command.
    line_hit: Option<(BufferId, usize)>,
    /// The selected row's preview.
    pub(super) preview: FilePreview,
}

impl Editor {
    /// Opens a prompt that searches the files of the project at `root`, named `name`, whose
    /// input is kept in the history `history`. It opens empty; each input that changes asks the
    /// host for a search ([`Editor::take_search_request`]), whose rows come to
    /// [`Editor::rows_found`].
    ///
    /// # Errors
    ///
    /// [`PromptError::TooDeep`] with as many prompts open as may be, and
    /// [`PromptError::Invalid`] for a root whose path is not UTF-8, as a row could not name a
    /// file under it, or while a search is open already. Nothing changes.
    pub fn open_project_search(
        &mut self,
        root: PathBuf,
        name: &str,
        history: &'static str,
    ) -> Result<PromptId, PromptError> {
        if root.to_str().is_none() || self.project_search.prompt.is_some() {
            return Err(PromptError::Invalid);
        }
        let style = CompletionStyle {
            given_order: true,
            preview: true,
            ..CompletionStyle::default()
        };
        let id =
            self.open_completion_as(format!("Search {name}:"), history, Arc::from([]), style)?;
        self.start_search_prompt(id, root, Source::Files);
        Ok(id)
    }

    /// Makes the search's state that of the new prompt `id`, of `source`, of the project at
    /// `root` for its files.
    fn start_search_prompt(&mut self, id: PromptId, root: PathBuf, source: Source) {
        let live = Arc::clone(&self.project_search.live);
        let mut preview = std::mem::take(&mut self.project_search.preview);
        preview.reset();
        let space_scope = self.project_search.space_scope.take();
        self.project_search = ProjectSearch {
            prompt: Some(id),
            root,
            live,
            preview,
            source,
            space_scope,
            ..ProjectSearch::default()
        };
    }

    /// The search of a project's files the host should start, once.
    pub fn take_search_request(&mut self) -> Option<ProjectSearchRequest> {
        match self.project_search.request.take() {
            Some(Request::Files(request)) => Some(request),
            other => {
                self.project_search.request = other;
                None
            }
        }
    }

    /// Takes in rows the search `generation` found, and with `end`, that it ended. They go
    /// after the rows shown, and the selected row stays selected. Rows of a search that is not
    /// the newest, or whose prompt has closed, are dropped.
    pub fn rows_found(
        &mut self,
        generation: u64,
        rows: Vec<ProjectSearchRow>,
        end: Option<WalkEnd>,
    ) {
        let search = &mut self.project_search;
        let Some(room) = search
            .prompt
            .filter(|_| generation == search.generation && search.running)
            .filter(|_| matches!(search.source, Source::Files))
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
            let shown = format!("{}:{}", row.path, row.line);
            let (candidate, prefix) = if row.secret {
                let prefix = shown.len();
                (Candidate::unmatched(shown, WITHHELD), prefix)
            } else {
                let prefix = shown.len() + 2;
                (
                    Candidate::unmatched(format!("{shown}: {}", row.text), ""),
                    prefix,
                )
            };
            search.places.push(Place {
                line: row.line,
                column: row.column,
                prefix,
                to: To::File {
                    path: row.path.len(),
                    offset: row.offset,
                    secret: row.secret,
                },
            });
            added.push(candidate);
        }
        self.add_search_rows(added, end, cut_here);
    }

    /// Puts `added` after the search's rows shown, whose places are kept already, and with
    /// `end`, ends the search; `cut_here` says more came than a search keeps.
    fn add_search_rows(&mut self, added: Vec<Candidate>, end: Option<WalkEnd>, cut_here: bool) {
        let search = &mut self.project_search;
        let Some(id) = search.prompt else {
            return;
        };
        let generation = search.generation;
        if let Some(end) = end {
            search.running = false;
            search.cut = end.cut;
        }
        if cut_here {
            // The host stops at its limit too; one that sent more is stopped here.
            search.live.fetch_add(1, Ordering::Relaxed);
            search.running = false;
            search.cut = true;
        }
        if !search.running && !search.cut {
            search.last_whole = search.query.clone().map(|query| (generation, query));
        }
        if let Some((candidates, _)) = self
            .prompts
            .iter_mut()
            .find(|prompt| prompt.id == id)
            .and_then(|prompt| prompt.completion.as_mut())
        {
            candidates.append(added);
        }
        self.relayout_prompt();
    }

    /// Whether the innermost prompt searches a project.
    pub(super) fn searches_project(&self) -> bool {
        self.project_search
            .prompt
            .is_some_and(|id| self.prompts.last().is_some_and(|prompt| prompt.id == id))
    }

    /// Whether the innermost prompt searches a project and its search runs.
    pub(super) fn search_pending(&self) -> bool {
        self.searches_project() && self.project_search.running
    }

    /// Whether the innermost prompt searches a project and its search stopped at a limit.
    pub(super) fn search_cut(&self) -> bool {
        self.searches_project() && self.project_search.cut
    }

    /// Starts a search for the innermost prompt's answer, if it searches a project and the
    /// answer changed since its last search: the rows go, and a search starts once the input
    /// says enough.
    pub(super) fn search_input_changed(&mut self) {
        let Some(prompt) = self.prompts.last() else {
            return;
        };
        // An answer too long to search for, such as a whole file yanked into it, is never read.
        let text = self
            .buffer(prompt.buffer)
            .map(|buffer| buffer.text())
            .filter(|text| text.len() <= MAX_INPUT_BYTES)
            .map(|text| text.to_string());
        if self.project_search.input.as_ref() == Some(&text) {
            return;
        }
        let migemo = self.migemo_starts_on();
        let around = self
            .prompts
            .last()
            .is_some_and(|prompt| prompt.around_secret);
        let cache = Cache::unless(around || self.shows_secret());
        let table = self.migemo.table();
        let search = &mut self.project_search;
        // Each component the input before had keeps what it compiled (ADR 0076).
        let query = Arc::new(match text.as_deref() {
            Some(text) => {
                let none = LineQuery::empty();
                let before = search.query.as_deref().unwrap_or(&none);
                LineQuery::parse_after(text, before, migemo, table.as_ref())
            }
            None => LineQuery::too_long(),
        });
        if query.wants_dictionary() {
            self.migemo.wanted_without(cache);
        }
        let search = &mut self.project_search;
        // A list of thousands is dropped on the worker (ADR 0096).
        let places = std::mem::take(&mut search.places);
        // The rows shown are all a search found that ended with no limit, which this one
        // narrows, so their lines hold all it can find.
        let whole_before = search.last_whole.as_ref().is_some_and(|(whole, before)| {
            *whole == search.generation && !search.running && query.narrows(before)
        });
        search.input = Some(text);
        search.generation = search.live.fetch_add(1, Ordering::Relaxed) + 1;
        search.preview.reset();
        search.cut = false;
        let generation = search.generation;
        let narrow = whole_before.then_some(&places[..]);
        let request = self.search_request(generation, &query, narrow);
        let search = &mut self.project_search;
        search.running = request.is_some();
        search.request = request;
        search.query = Some(query);
        if places.len() > WORKER_PAST {
            self.free_on_the_worker(Box::new(places));
        }
        if let Some((candidates, _)) = self.prompts.last_mut().and_then(|p| p.completion.as_mut()) {
            let mut empty = Candidates::in_given_order(Arc::from([]));
            // The float stays split for the preview (ADR 0114).
            empty.show_preview();
            let rows = std::mem::replace(candidates, empty);
            self.free_candidates(rows);
        }
    }

    /// The search the host should start for `query`, the input that changed, as the search
    /// `generation`, if one starts: with `narrowed`, the rows of the search before, which hold
    /// every line this one can find.
    fn search_request(
        &mut self,
        generation: u64,
        query: &Arc<LineQuery>,
        narrowed: Option<&[Place]>,
    ) -> Option<Request> {
        let search = &self.project_search;
        match &search.source {
            Source::Files => query.starts().then(|| {
                Request::Files(ProjectSearchRequest {
                    generation,
                    root: search.root.clone(),
                    query: Arc::clone(query),
                    live: Arc::clone(&search.live),
                    narrow: search
                        .last_whole
                        .as_ref()
                        .filter(|(_, before)| query.narrows(before))
                        .map(|(generation, _)| *generation),
                })
            }),
            Source::Conversations(conversations) => query.starts().then(|| {
                Request::Conversations(ConversationSearchRequest {
                    generation,
                    scope: conversations.scope().clone(),
                    query: Arc::clone(query),
                    live: Arc::clone(&search.live),
                })
            }),
            // A buffer's lines are searched for every input, the empty one too.
            Source::Lines(_) => (!query.is_too_long())
                .then(|| self.line_search_request(generation, query, narrowed))
                .flatten()
                .map(Request::Lines),
        }
    }

    /// Searches again if the innermost prompt searches a project for a migemo component that
    /// took its words from another dictionary than the one migemo has now, as one came or
    /// went (ADR 0116): its words change what it matches.
    pub(super) fn project_search_look_again(&mut self) {
        let generation = crate::migemo::generation(self.migemo.table().as_ref());
        let stale = (self.project_search.query.as_ref())
            .is_some_and(|query| query.compiled_for_another(generation));
        if !self.searches_project() || !stale {
            return;
        }
        self.project_search.input = None;
        self.search_input_changed();
        self.relayout_prompt();
    }

    /// The whole path of the innermost prompt's selected row, if it searches a project, and the
    /// input, which its history keeps, unless the row is of a file that may hold secrets, whose
    /// lines the input matches: the row's place is kept for the command
    /// ([`Editor::take_search_hit`]).
    pub(super) fn search_answer(&mut self) -> Option<(String, Option<String>)> {
        if !self.searches_project() {
            return None;
        }
        let prompt = self.prompts.last()?;
        let candidates = prompt.candidates()?;
        let item = candidates.selected_item()?;
        let text = candidates.get(candidates.selected()?)?.text();
        // A row of a buffer gone since gives nothing, never its text, which the history would
        // keep.
        if self.project_search.source.lines().is_some() {
            return Some(self.line_answer(item).unwrap_or_default());
        }
        let search = &mut self.project_search;
        let place = search.places.get(item)?;
        if let To::Message { session, message } = &place.to {
            search.conversation_hit = Some(ConversationHit {
                session: session.clone(),
                message: *message,
            });
            let input = search.input.clone().flatten().unwrap_or_default();
            return Some((session.to_string(), Some(input.trim().to_owned())));
        }
        let path = search.root.join(text.get(..place.path())?);
        let answer = path.to_str()?.to_owned();
        search.hit = Some(ProjectSearchHit {
            path,
            line: place.line,
            column: place.column,
        });
        let input = search.input.clone().flatten().unwrap_or_default();
        let recorded = (!place.secret()).then(|| input.trim().to_owned());
        Some((answer, recorded))
    }

    /// The row the last search's `RET` took, once.
    pub fn take_search_hit(&mut self) -> Option<ProjectSearchHit> {
        self.project_search.hit.take()
    }

    /// Where a search's row of `text`, shown at `place` of the innermost prompt, matches, into
    /// `out` as byte ranges of `text`, if it searches a project. Gives how many bytes of `text`
    /// are its path and line, which show in the annotation's face; a row of a file that may
    /// hold secrets is all of them, and matches nowhere.
    pub fn search_highlights(
        &self,
        place: usize,
        text: &str,
        out: &mut Vec<Range<usize>>,
    ) -> Option<usize> {
        out.clear();
        if !self.searches_project() {
            return None;
        }
        let item = self.prompts.last()?.candidates()?.item_at(place)?;
        let row = self.project_search.places.get(item)?;
        let prefix = row.prefix.min(text.len());
        if row.secret() || !text.is_char_boundary(prefix) {
            return Some(prefix);
        }
        if let Some(query) = &self.project_search.query {
            query.highlights(&text[prefix..], out);
            for range in out.iter_mut() {
                *range = range.start + prefix..range.end + prefix;
            }
        }
        Some(prefix)
    }

    /// Forgets the prompt that searched a project, once it has closed: its search stops at its
    /// next file or block.
    pub(super) fn forget_project_search(&mut self, closed: PromptId) {
        if self.project_search.prompt == Some(closed) {
            self.project_search.live.fetch_add(1, Ordering::Relaxed);
            let live = Arc::clone(&self.project_search.live);
            let hit = self.project_search.hit.take();
            let conversation_hit = self.project_search.conversation_hit.take();
            let line_hit = self.project_search.line_hit.take();
            let space_scope = self.project_search.space_scope.take();
            let mut preview = std::mem::take(&mut self.project_search.preview);
            preview.reset();
            self.project_search = ProjectSearch {
                live,
                hit,
                conversation_hit,
                line_hit,
                space_scope,
                preview,
                ..ProjectSearch::default()
            };
        }
    }
}

#[cfg(test)]
mod tests;
