//! What the lists a language server's work fills share (ADR 0138, steps 6b and 8b): `M-g d`'s
//! list of diagnostics and the list of a server's places. Each is a prompt that opens waiting,
//! whose rows the host builds and hands over, with the preview of each row's place, in an open
//! buffer or in its file, and `RET`'s row kept for whoever goes there.
//!
//! Must not depend on: `command` or `keymap`, or the host's servers, threads or I/O; the host
//! builds the rows and reads a file the preview asks for.
//!
//! - A row of an open buffer goes to its place as the buffer's edits since have moved it, as
//!   the cursors move, clipped to its text; once the buffer's edits no longer reach the list's
//!   revision, or the buffer has gone, the row is its file's, by line.
//! - What was typed while the rows came is matched against them, as a new input is.

use std::cell::Cell;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use crate::buffer::BufferId;
use crate::complete::{Candidate, Candidates, WORKER_PAST};
use crate::editor::Editor;
use crate::editor::file_preview::FilePreview;
use crate::editor::visits::VisitColumn;
use crate::minibuffer::PromptId;
use crate::selection::{OffsetEdit, follow_offset};
use crate::sum_tree::Bias;

/// Where a row's place is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListedAt {
    /// In the open buffer `buffer`, at `offset` of its text at `revision`.
    Open {
        buffer: BufferId,
        offset: usize,
        revision: u64,
    },
    /// In a file no buffer held, at `column` of its line, as its server counts it.
    File { column: VisitColumn },
}

/// The row `RET` took: an open buffer's place, or a file's line and column.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListedHit {
    /// The open buffer `id`, at `offset` of its text now.
    Buffer { id: BufferId, offset: usize },
    /// The file at `path`, at `line`, from 1, and `column` of it.
    File {
        path: PathBuf,
        line: usize,
        column: VisitColumn,
    },
}

/// What a list keeps beside its rows, each row's place a `P`. `Debug` gives its generation.
pub(in crate::editor) struct Listed<P> {
    pub(super) prompt: Option<PromptId>,
    /// The root its files are under, as their rows show them.
    pub(super) root: PathBuf,
    /// The list the rows are for, which moves with each list opened.
    pub(super) generation: u64,
    /// The rows have not come.
    pub(super) pending: bool,
    /// The rows were cut at a limit.
    pub(super) cut: bool,
    /// Each row's place, by its index in the list.
    pub(super) places: Vec<P>,
    /// The row `RET` took.
    pub(super) hit: Option<ListedHit>,
    /// The selected row's preview of a file no buffer holds.
    pub(in crate::editor) preview: FilePreview,
    /// Where an open buffer's row was last found, as the preview asks each frame: its item, the
    /// buffer's revision then, and its offset, or `None` once the buffer's edits no longer
    /// reach the list's.
    moved: Cell<Option<(usize, u64, Option<usize>)>>,
}

impl<P> fmt::Debug for Listed<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Listed")
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

impl<P> Default for Listed<P> {
    fn default() -> Self {
        Self {
            prompt: None,
            root: PathBuf::new(),
            generation: 0,
            pending: false,
            cut: false,
            places: Vec::new(),
            hit: None,
            preview: FilePreview::default(),
            moved: Cell::new(None),
        }
    }
}

impl<P> Listed<P> {
    /// The list opens as `prompt`, `generation`, over `root`, waiting for its rows; the
    /// preview's own allocations are kept.
    pub(super) fn open(&mut self, prompt: PromptId, generation: u64, root: PathBuf) {
        let mut preview = std::mem::take(&mut self.preview);
        preview.reset();
        *self = Self {
            prompt: Some(prompt),
            root,
            generation,
            pending: true,
            preview,
            ..Self::default()
        };
    }

    /// The prompt the rows of `generation` go to, if they are for the list open and waiting.
    pub(super) fn waiting_for(&self, generation: u64) -> Option<PromptId> {
        self.prompt
            .filter(|_| self.pending && generation == self.generation)
    }

    /// Takes the rows' `places`, and whether they were `cut`.
    pub(super) fn take(&mut self, places: Vec<P>, cut: bool) {
        self.pending = false;
        self.cut = cut;
        self.places = places;
        self.moved.set(None);
    }

    /// Forgets the list once its prompt `closed`, keeping its generation and the row `RET`
    /// took; its places come back, for the caller to free.
    pub(super) fn close(&mut self, closed: PromptId) -> Option<Vec<P>> {
        if self.prompt != Some(closed) {
            return None;
        }
        let mut preview = std::mem::take(&mut self.preview);
        preview.reset();
        let places = std::mem::take(&mut self.places);
        let (generation, hit) = (self.generation, self.hit.take());
        *self = Self {
            generation,
            hit,
            preview,
            ..Self::default()
        };
        Some(places)
    }

    /// Where the open buffer of the row `item`, at `at`, is now in `editor`; `None` for a
    /// file's row and once its place is no longer known. Worked out once for each revision of
    /// the buffer.
    pub(super) fn open_now(
        &self,
        editor: &Editor,
        item: usize,
        at: ListedAt,
    ) -> Option<(BufferId, usize)> {
        let ListedAt::Open {
            buffer: id,
            offset,
            revision,
        } = at
        else {
            return None;
        };
        let buffer = editor.buffer(id)?;
        let now = buffer.revision();
        let offset = if now == revision {
            offset
        } else if let Some((seen, at, kept)) = self.moved.get()
            && (seen, at) == (item, now)
        {
            kept?
        } else {
            let found = buffer.edits_since(revision).map(|splices| {
                let mut edits: Vec<OffsetEdit> = (splices.iter())
                    .map(|splice| OffsetEdit {
                        start: splice.start,
                        old_len: splice.old_len,
                        new_len: splice.new_len,
                    })
                    .collect();
                edits.sort_unstable_by_key(|edit| edit.start);
                follow_offset(offset, &edits, Bias::Left)
            });
            self.moved.set(Some((item, now, found)));
            found?
        };
        let text = buffer.text();
        Some((id, text.clip_offset(offset.min(text.len()), Bias::Left)))
    }

    /// The hit of the row `item`, at `at`, of the file at `path`, line `line`: its open buffer's
    /// place while it is known, else the file's line and column.
    pub(super) fn hit_of(
        &self,
        editor: &Editor,
        (item, at): (usize, ListedAt),
        (path, line): (PathBuf, usize),
    ) -> ListedHit {
        match self.open_now(editor, item, at) {
            Some((id, offset)) => ListedHit::Buffer { id, offset },
            None => ListedHit::File {
                path,
                line,
                column: file_column(at),
            },
        }
    }

    /// Whether `editor`'s innermost prompt is this list's.
    pub(super) fn innermost(&self, editor: &Editor) -> bool {
        self.prompt
            .is_some_and(|id| editor.prompts.last().is_some_and(|prompt| prompt.id == id))
    }
}

/// The column a row of `at` is gone to by its file: its server's, or the line's start for an
/// open buffer's row whose place is no longer known.
pub(super) fn file_column(at: ListedAt) -> VisitColumn {
    match at {
        ListedAt::File { column } => column,
        ListedAt::Open { .. } => VisitColumn::Chars(0),
    }
}

impl Editor {
    /// Shows `items`, a list's rows, in its prompt `id`, with `selected` selected unless
    /// something was typed while they came, which is matched against them; the rows are freed
    /// on the worker if the prompt has gone.
    pub(super) fn show_listed(
        &mut self,
        id: PromptId,
        items: Arc<[Candidate]>,
        selected: Option<usize>,
    ) {
        let typed = (self.prompts.iter().find(|prompt| prompt.id == id))
            .and_then(|prompt| self.buffer(prompt.buffer))
            .is_some_and(|buffer| !buffer.text().is_empty());
        let completion = (self.prompts.iter_mut().find(|prompt| prompt.id == id))
            .and_then(|prompt| prompt.completion.as_mut());
        let Some((candidates, seen)) = completion else {
            if items.len() > WORKER_PAST {
                self.free_on_the_worker(Box::new(items));
            }
            return;
        };
        let mut listed = Candidates::in_given_order(items);
        listed.show_preview();
        if let Some(selected) = selected.filter(|_| !typed) {
            listed.select(selected);
        }
        if typed {
            *seen = u64::MAX;
        }
        let empty = std::mem::replace(candidates, listed);
        self.free_candidates(empty);
        self.match_completion();
        self.relayout_prompt();
    }
}
