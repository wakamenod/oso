//! A project's search for `project-search` (ADR 0114): the prompt whose input is what to look
//! for, the search the host runs for each input, and the rows it sends, put after the rows shown
//! as they come.
//!
//! Must not depend on: `command` or `keymap`, or the host's threads or I/O; the host searches
//! the files and hands over the rows it found.
//!
//! Each input that changes starts a search with a new generation, once a component that is not
//! excluded has two characters; the one before stops at its next file or block. An input that
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

use super::{CompletionStyle, Editor, WalkEnd};
use crate::complete::{Candidate, Candidates, MAX_INPUT_BYTES, WORKER_PAST};
use crate::minibuffer::{PromptError, PromptId};
use crate::project::{LineQuery, MAX_ROWS};
use preview::SearchPreview;
pub use preview::{Excerpt, ProjectPreviewRead, ProjectPreviewRequest};

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

/// What a row keeps beside its candidate: where it leads, and how much of its text is its
/// path and line. The path itself is read from the candidate's text, which starts with it.
struct Place {
    /// The bytes of the path at the start of the candidate's text.
    path: usize,
    line: usize,
    /// The offset of the line in its file, as the search found it.
    offset: u64,
    column: usize,
    /// The bytes of `path:line: `, or the whole text for a file that may hold secrets.
    prefix: usize,
    secret: bool,
}

/// What the prompt that searches a project keeps beside its rows.
#[derive(Default)]
pub(super) struct ProjectSearch {
    prompt: Option<PromptId>,
    root: PathBuf,
    /// The generation of the search the rows are of.
    generation: u64,
    /// The newest search's generation, shared with the host, which stops once it moves on.
    live: Arc<AtomicU64>,
    /// The search to start, until the host takes it.
    request: Option<ProjectSearchRequest>,
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
    /// The selected row's preview.
    preview: SearchPreview,
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
        let live = Arc::clone(&self.project_search.live);
        let mut preview = std::mem::take(&mut self.project_search.preview);
        preview.reset();
        self.project_search = ProjectSearch {
            prompt: Some(id),
            root,
            live,
            preview,
            ..ProjectSearch::default()
        };
        Ok(id)
    }

    /// The search the host should start, once.
    pub fn take_search_request(&mut self) -> Option<ProjectSearchRequest> {
        self.project_search.request.take()
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
        let Some(id) = search
            .prompt
            .filter(|_| generation == search.generation && search.running)
        else {
            if rows.len() > WORKER_PAST {
                self.free_on_the_worker(Box::new(rows));
            }
            return;
        };
        let room = MAX_ROWS.saturating_sub(search.places.len());
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
                path: row.path.len(),
                line: row.line,
                offset: row.offset,
                column: row.column,
                prefix,
                secret: row.secret,
            });
            added.push(candidate);
        }
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
        let search = &mut self.project_search;
        if search.input.as_ref() == Some(&text) {
            return;
        }
        let query = Arc::new(
            text.as_deref()
                .map_or_else(LineQuery::too_long, LineQuery::parse),
        );
        search.input = Some(text);
        search.generation = search.live.fetch_add(1, Ordering::Relaxed) + 1;
        search.preview.reset();
        // A list of thousands is dropped on the worker (ADR 0096).
        let places = std::mem::take(&mut search.places);
        search.cut = false;
        search.running = query.starts();
        search.request = search.running.then(|| ProjectSearchRequest {
            generation: search.generation,
            root: search.root.clone(),
            query: Arc::clone(&query),
            live: Arc::clone(&search.live),
            narrow: search
                .last_whole
                .as_ref()
                .filter(|(_, before)| query.narrows(before))
                .map(|(generation, _)| *generation),
        });
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
        let search = &mut self.project_search;
        let place = search.places.get(item)?;
        let path = search.root.join(text.get(..place.path)?);
        let answer = path.to_str()?.to_owned();
        search.hit = Some(ProjectSearchHit {
            path,
            line: place.line,
            column: place.column,
        });
        let input = search.input.clone().flatten().unwrap_or_default();
        let recorded = (!place.secret).then(|| input.trim().to_owned());
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
        if row.secret || !text.is_char_boundary(prefix) {
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
            let mut preview = std::mem::take(&mut self.project_search.preview);
            preview.reset();
            self.project_search = ProjectSearch {
                live,
                hit,
                preview,
                ..ProjectSearch::default()
            };
        }
    }
}

#[cfg(test)]
mod tests;
