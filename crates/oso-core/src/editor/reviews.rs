//! The open reviews (ADR 0122, "The views"): each one's model, its review buffer and the rows
//! that say what each of its lines is, and what commands ask of the host for them. What the
//! review menu shows is in `menu`, and the backgrounds of a review's rows in `cells`.
//!
//! Must not depend on: `command` or `keymap`, or the host's threads, git or files; the host
//! runs git and builds each review's text on threads of its own, and hands the text in.
//!
//! - A review buffer is internal and read-only, in `Review` mode, and truncates its lines. Its
//!   text comes whole from the host's view thread, with its display map and highlights, and is
//!   put in at O(1) as a file read again is, with every cursor and window top carried through
//!   the one span that changed and no undo.
//! - A new text waits while [`Editor::review_waits`], as a file read again does, so nothing a
//!   prompt or a search acts on moves under it. The host keeps the texts that wait, in order.
//! - What a line is comes from its [`Rows`], never from its text.
//! - What a new text or paint replaces comes back as a [`ReviewLeftover`], for the host to
//!   free off the UI thread, as a 64 MiB text's highlights take milliseconds to drop.

mod cells;
mod menu;

use super::Editor;
use crate::buffer::{BufferId, Mode, Rewrap};
use crate::load::{Change, Reloaded};
use crate::project::ProjectId;
use crate::review::{Author, Review, Rows, Side};
use crate::sum_tree::Bias;
use crate::syntax::Highlights;

pub use cells::ReviewCells;
pub use menu::{Count, ScopeMenu};

/// A review's id, never reused while the editor runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ReviewId(u64);

impl ReviewId {
    /// The id as a number.
    pub fn get(self) -> u64 {
        self.0
    }
}

/// What a review compares, as the menu offers it (ADR 0122, "What can be reviewed").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ScopeKind {
    Session,
    Uncommitted,
    Unstaged,
    Staged,
    Branch,
    Commit,
    Range,
    PullRequest,
}

impl ScopeKind {
    /// How many scopes there are, which [`ScopeKind::index`] numbers from 0.
    pub(super) const COUNT: usize = 8;

    /// Where the scope is kept in a list of [`ScopeKind::COUNT`].
    pub(super) fn index(self) -> usize {
        match self {
            Self::Session => 0,
            Self::Uncommitted => 1,
            Self::Unstaged => 2,
            Self::Staged => 3,
            Self::Branch => 4,
            Self::Commit => 5,
            Self::Range => 6,
            Self::PullRequest => 7,
        }
    }
}

/// What commands ask of the host for reviews.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReviewRequest {
    /// Open a review of `scope` in `project`; `revision` is a commit's, a range's `FROM..TO`
    /// or the branch's base, as the person gave it.
    Open {
        project: ProjectId,
        scope: ScopeKind,
        revision: Option<String>,
    },
    /// Take a baseline of the project's work tree for the session scope.
    Baseline { project: ProjectId },
    /// Read the review's files again now.
    Refresh(ReviewId),
    /// Build the file's section again, as its comments changed.
    Redraw { review: ReviewId, path: Box<[u8]> },
    /// Go to line `number` of `side` of the file in its source, or in its old side when
    /// `old_side`.
    Visit {
        review: ReviewId,
        path: Box<[u8]>,
        side: Side,
        number: u32,
        old_side: bool,
    },
    /// The review was closed: stop its threads.
    Close(ReviewId),
}

/// One open review.
#[derive(Debug)]
struct Open {
    id: ReviewId,
    project: ProjectId,
    buffer: BufferId,
    review: Review,
    rows: Rows,
    agents_folded: bool,
    /// The files read and listed while a request reads them.
    reading: Option<(usize, usize)>,
}

/// Every open review, what commands asked of the host, and the menu's state per project.
#[derive(Debug, Default)]
pub(super) struct Reviews {
    next: u64,
    open: Vec<Open>,
    requests: Vec<ReviewRequest>,
    menus: Vec<(ProjectId, ScopeMenu)>,
}

impl Reviews {
    fn get(&self, id: ReviewId) -> Option<&Open> {
        self.open.iter().find(|open| open.id == id)
    }

    fn get_mut(&mut self, id: ReviewId) -> Option<&mut Open> {
        self.open.iter_mut().find(|open| open.id == id)
    }
}

/// What putting in a review's text or paint left over: the rows, highlights and text it
/// replaced or did not take. They may be large, so the host frees them off the UI thread.
#[must_use = "free it off the UI thread"]
#[derive(Default)]
pub struct ReviewLeftover {
    rows: Option<Rows>,
    highlights: Option<Highlights>,
    text: Option<Box<Reloaded>>,
}

impl ReviewLeftover {
    fn of(rows: Rows, highlights: Highlights, text: Option<Reloaded>) -> ReviewLeftover {
        ReviewLeftover {
            rows: Some(rows),
            highlights: Some(highlights),
            text: text.map(Box::new),
        }
    }
}

#[cfg(test)]
impl ReviewLeftover {
    /// Whether it holds rows, highlights and a text.
    pub(crate) fn holds(&self) -> (bool, bool, bool) {
        (
            self.rows.is_some(),
            self.highlights.is_some(),
            self.text.is_some(),
        )
    }
}

impl std::fmt::Debug for ReviewLeftover {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReviewLeftover")
            .field("rows", &self.rows.is_some())
            .field("highlights", &self.highlights.is_some())
            .field("text", &self.text.is_some())
            .finish()
    }
}

impl Editor {
    /// Asks the host for `request`.
    pub fn request_review(&mut self, request: ReviewRequest) {
        self.reviews.requests.push(request);
    }

    /// What commands asked of the host for reviews, oldest first.
    pub fn take_review_requests(&mut self) -> Vec<ReviewRequest> {
        std::mem::take(&mut self.reviews.requests)
    }

    /// Opens a review of `project` with an empty review buffer named `name`, shown in no window
    /// yet. The buffer truncates its lines.
    pub fn add_review(&mut self, project: ProjectId, name: String) -> (ReviewId, BufferId) {
        let id = ReviewId(self.reviews.next);
        self.reviews.next += 1;
        let buffer = self.add_internal_buffer(name, Mode::Review);
        if let Some(shown) = self.buffer_mut(buffer) {
            shown.set_wraps(false);
        }
        self.reviews.open.push(Open {
            id,
            project,
            buffer,
            review: Review::new(),
            rows: Rows::default(),
            agents_folded: false,
            reading: None,
        });
        (id, buffer)
    }

    /// Closes the review `id`: its buffer goes, every window that showed it going back to
    /// where it was, and the host is asked to stop its threads.
    pub fn remove_review(&mut self, id: ReviewId) {
        let Some(index) = self.reviews.open.iter().position(|open| open.id == id) else {
            return;
        };
        let open = self.reviews.open.remove(index);
        self.drop_internal_buffer(open.buffer);
        self.reviews.requests.push(ReviewRequest::Close(id));
    }

    /// Every open review and its project, oldest first.
    pub fn reviews(&self) -> impl Iterator<Item = (ReviewId, ProjectId)> {
        self.reviews.open.iter().map(|open| (open.id, open.project))
    }

    pub fn review(&self, id: ReviewId) -> Option<&Review> {
        Some(&self.reviews.get(id)?.review)
    }

    pub fn review_mut(&mut self, id: ReviewId) -> Option<&mut Review> {
        Some(&mut self.reviews.get_mut(id)?.review)
    }

    /// The review buffer of `id`.
    pub fn review_buffer(&self, id: ReviewId) -> Option<BufferId> {
        Some(self.reviews.get(id)?.buffer)
    }

    /// The review whose buffer is `buffer`.
    pub fn review_of(&self, buffer: BufferId) -> Option<ReviewId> {
        let open = self
            .reviews
            .open
            .iter()
            .find(|open| open.buffer == buffer)?;
        Some(open.id)
    }

    /// What each line of the review buffer of `id` is.
    pub fn review_rows(&self, id: ReviewId) -> Option<&Rows> {
        Some(&self.reviews.get(id)?.rows)
    }

    /// Whether a review's new text must wait, as a file read again does
    /// ([`Editor::reload_waits`]): the host keeps it, and the ones after it, until this is
    /// false, and puts them in in order.
    pub fn review_waits(&self) -> bool {
        self.reload_waits()
    }

    /// Puts in the review buffer's whole new text, built on the host's view thread and
    /// compared there with the text it replaces, with what each line is and its highlights.
    /// O(1) and the cursors: every window's cursors and top keep their place outside the span
    /// that changed and their line and column inside it, as a reload's do, with no undo. The
    /// host calls it only while [`Editor::review_waits`] is false, keeps the text before at
    /// O(1), and frees it and what this returns off the UI thread.
    pub fn set_review_text(
        &mut self,
        id: ReviewId,
        text: Reloaded,
        rows: Rows,
        highlights: Highlights,
    ) -> ReviewLeftover {
        let Some(buffer_id) = self.review_buffer(id) else {
            return ReviewLeftover::of(rows, highlights, Some(text));
        };
        let Some(buffer) = self.buffer(buffer_id) else {
            return ReviewLeftover::of(rows, highlights, Some(text));
        };
        let Reloaded { snapshot, change } = text;
        let old = buffer.text().clone();
        let change = match change {
            None if snapshot.display.invalid_bytes().is_empty()
                && buffer.invalid_bytes().is_empty() =>
            {
                let mut left = ReviewLeftover {
                    text: Some(Box::new(Reloaded { snapshot, change })),
                    ..ReviewLeftover::default()
                };
                left.highlights = self.put_review_highlights(buffer_id, highlights);
                left.rows = self.put_review_rows(id, rows);
                return left;
            }
            None => Change {
                start: 0,
                old_end: old.len(),
                new_end: snapshot.text.len(),
            },
            Some(change) => change,
        };
        let display = snapshot.display.snapshot();
        let moved = self.reloaded_selections(buffer_id, &old, &snapshot.text, &display, change);
        let mut views = self.moved_views(buffer_id, &display, &[], &Rewrap::Whole);
        for (_, _, scroll) in &mut views {
            let start = super::reload::through_reload(scroll.start, &old, &snapshot.text, change);
            let point = display.offset_to_display_point(start);
            scroll.start = display.display_point_to_offset(point, Bias::Left);
        }
        // Nothing past this point can panic part way.
        let mut left = ReviewLeftover::default();
        if let Some(buffer) = self.buffer_mut(buffer_id) {
            // Taken first, as the reload would drop them here.
            let old_highlights = buffer.take_generated_highlights();
            buffer.replace_with_reload(snapshot);
            left.highlights = match buffer.set_generated_highlights(highlights) {
                Some(refused) => Some(refused),
                None => old_highlights,
            };
        }
        left.rows = self.put_review_rows(id, rows);
        self.commit_views(buffer_id, views);
        for (index, set) in moved {
            let scroll = self.windows[index].scroll();
            self.windows[index].restore_positions(set, scroll, &display);
        }
        let shown: Vec<usize> = (0..self.windows.len())
            .filter(|index| self.windows[*index].buffer() == buffer_id)
            .collect();
        for index in shown {
            self.follow_window(index);
        }
        left
    }

    /// Puts in what each line is and the highlights for a text that did not change, as when
    /// refinement found more to mark, and returns what they replaced.
    ///
    /// # Errors
    ///
    /// The rows and highlights given back, changing nothing, when the rows do not cover the
    /// buffer's lines or the highlights its text.
    pub fn set_review_paint(
        &mut self,
        id: ReviewId,
        rows: Rows,
        highlights: Highlights,
    ) -> Result<ReviewLeftover, ReviewLeftover> {
        let buffer = self
            .review_buffer(id)
            .and_then(|id| Some((id, self.buffer(id)?)));
        let Some((buffer_id, buffer)) = buffer else {
            return Err(ReviewLeftover::of(rows, highlights, None));
        };
        let lines = buffer.text().max_point().row + 1;
        if rows.lines() + 1 < lines || highlights.len() != buffer.text().len() {
            return Err(ReviewLeftover::of(rows, highlights, None));
        }
        Ok(ReviewLeftover {
            highlights: self.put_review_highlights(buffer_id, highlights),
            rows: self.put_review_rows(id, rows),
            text: None,
        })
    }

    /// Puts in a review buffer's highlights, returning the ones before, or these if refused.
    fn put_review_highlights(
        &mut self,
        buffer: BufferId,
        highlights: Highlights,
    ) -> Option<Highlights> {
        let Some(buffer) = self.buffer_mut(buffer) else {
            return Some(highlights);
        };
        let old = buffer.take_generated_highlights();
        buffer.set_generated_highlights(highlights).or(old)
    }

    /// Puts in what each line of the review `id` is, returning the rows before.
    fn put_review_rows(&mut self, id: ReviewId, rows: Rows) -> Option<Rows> {
        match self.reviews.get_mut(id) {
            Some(open) => Some(std::mem::replace(&mut open.rows, rows)),
            None => Some(rows),
        }
    }

    /// Whether `a` folds every agent card of the review `id` to its first line.
    pub fn agents_folded(&self, id: ReviewId) -> bool {
        self.reviews.get(id).is_some_and(|open| open.agents_folded)
    }

    /// Folds every agent card of the review `id` to its first line, or shows them whole.
    pub fn fold_agents(&mut self, id: ReviewId, folded: bool) {
        if let Some(open) = self.reviews.get_mut(id) {
            open.agents_folded = folded;
        }
    }

    /// The paths of the files the windows showing review `id`'s buffer show some line of, into
    /// `out`, emptied first, each once, in the review's order, for the host to read them first.
    pub fn shown_review_files(&self, id: ReviewId, out: &mut Vec<Box<[u8]>>) {
        out.clear();
        let Some(open) = self.reviews.get(id) else {
            return;
        };
        let Some(buffer) = self.buffer(open.buffer) else {
            return;
        };
        let display = buffer.display();
        for window in self.windows.iter().filter(|w| w.buffer() == open.buffer) {
            let Some(regions) = self.regions(window.id()) else {
                continue;
            };
            let top = display.offset_to_display_point(window.scroll().start).row;
            for line in top..top + regions.text.rows {
                let Some(row) = open.rows.at(line) else {
                    break;
                };
                if out.last().is_none_or(|last| **last != *row.path) {
                    out.push(row.path.into());
                }
            }
        }
        out.sort_unstable();
        out.dedup();
    }

    /// Says how many of the files listed the review `id`'s running request has read, for its
    /// modeline, or `None` once it read them all.
    pub fn set_review_reading(&mut self, id: ReviewId, reading: Option<(usize, usize)>) {
        if let Some(open) = self.reviews.get_mut(id) {
            open.reading = reading;
        }
    }

    /// What a review buffer's modeline says after its mode, `Review`, into `out`: how many agent
    /// comments are unread and how far a reading is. Nothing for another buffer. Costs a look at
    /// each comment.
    pub(super) fn review_status_into(&self, buffer: BufferId, out: &mut String) {
        use std::fmt::Write as _;
        let Some(open) = self.reviews.open.iter().find(|open| open.buffer == buffer) else {
            return;
        };
        let unread = open
            .review
            .comments()
            .filter(|comment| comment.author() != Author::Person && !comment.is_read())
            .count();
        if unread > 0 {
            let _ = write!(out, "{unread} unread  ");
        }
        if let Some((read, listed)) = open.reading {
            let _ = write!(out, "reading {read}/{listed}  ");
        }
    }
}
