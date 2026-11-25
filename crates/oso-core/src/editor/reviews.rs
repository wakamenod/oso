//! The open reviews (ADR 0122, "The views"): each one's model, its review buffer and the rows
//! that say what each of its lines is, and what commands ask of the host for them. What the
//! review menu shows is in `menu`, the backgrounds of a review's rows in `cells`, and its two
//! panes in `panes`.
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
//! - A file whose shown reading the model has not caught up with is marked behind by the host
//!   ([`Editor::set_review_file_behind`]) until its carry is applied, and commands that write a
//!   comment on it refuse meanwhile.
//! - What a new text or paint replaces comes back as a [`ReviewLeftover`], for the host to
//!   free off the UI thread, as a 64 MiB text's highlights take milliseconds to drop.

mod cells;
mod commits;
mod draft;
mod files;
mod kept;
mod leftover;
mod menu;
mod panes;
mod places;
mod request;
mod status;

pub use leftover::ReviewLeftover;

use std::collections::BTreeSet;

use super::Editor;
use crate::buffer::{BufferId, Mode, Rewrap};
use crate::load::{Change, Reloaded};
use crate::project::ProjectId;
use crate::review::{Review, RowKind, Rows};
use crate::sum_tree::Bias;
use crate::syntax::Highlights;

pub use cells::ReviewCells;
pub use commits::{CommitEntry, Commits, Since};
pub use draft::DraftFor;
pub use menu::{Count, ScopeMenu};
pub use panes::{ReviewArea, ReviewChoice, ReviewLayout};
pub use places::LineMoves;
pub use request::{ReviewRequest, ReviewStart, Step, VisitTo};

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
    /// The files whose reading shown is newer than the model's, until their carry is applied.
    behind: BTreeSet<Box<[u8]>>,
    /// How the person chose it to show, and its two panes while the host builds them (ADR
    /// 0122, step 11).
    choice: ReviewChoice,
    panes: Option<panes::Panes>,
    /// The rows of panes dropped since, for the next leftover to take off the UI thread.
    spare: Vec<Rows>,
    /// Whether the host keeps the review across a restart, saving it as it changes (ADR 0122,
    /// step 6): its comments are then not lost with the editor.
    kept: bool,
    /// Its files pane and its filter (step 6b).
    files: files::Files,
    /// A pull request's commits and their list (ADR 0167).
    walk: commits::Walk,
    /// The pull request review this one reviews a commit of, and that commit's index.
    pull: Option<(ReviewId, commits::Link)>,
}

impl Open {
    /// What each line of `buffer` is, its review buffer or a pane of it.
    fn rows_of(&self, buffer: BufferId) -> Option<&Rows> {
        if self.buffer == buffer {
            return Some(&self.rows);
        }
        let panes = self.panes.as_ref()?;
        let side = panes.buffers.iter().position(|pane| *pane == buffer)?;
        Some(&panes.rows[side])
    }
}

/// Every open review, what commands asked of the host, and the menu's state per project.
#[derive(Debug, Default)]
pub(super) struct Reviews {
    next: u64,
    open: Vec<Open>,
    requests: Vec<ReviewRequest>,
    menus: Vec<(ProjectId, ScopeMenu)>,
    /// The open draft, if any.
    draft: Option<draft::Draft>,
    /// The models of reviews closed since the host last took them, for it to save and free.
    closed: Vec<(ReviewId, Review)>,
    /// What the files panes keep beside every review (step 6b).
    files: files::Shared,
    /// The project the review menu is for in place of the selected buffer's, while a start in
    /// a space chose it (ADR 0163).
    target: Option<ProjectId>,
}

impl Reviews {
    fn get(&self, id: ReviewId) -> Option<&Open> {
        self.open.iter().find(|open| open.id == id)
    }

    fn get_mut(&mut self, id: ReviewId) -> Option<&mut Open> {
        self.open.iter_mut().find(|open| open.id == id)
    }

    /// The review whose buffer, or one of whose panes, is `buffer`.
    fn holding(&self, buffer: BufferId) -> Option<&Open> {
        self.open.iter().find(|open| {
            open.buffer == buffer
                || open
                    .panes
                    .as_ref()
                    .is_some_and(|panes| panes.buffers.contains(&buffer))
        })
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
            behind: BTreeSet::new(),
            choice: ReviewChoice::Auto,
            panes: None,
            spare: Vec::new(),
            kept: false,
            files: files::Files::default(),
            walk: commits::Walk::default(),
            pull: None,
        });
        (id, buffer)
    }

    /// Closes the review `id`: its buffer goes, every window that showed it going back to
    /// where it was, and the host is asked to stop its threads.
    pub fn remove_review(&mut self, id: ReviewId) {
        self.close_review_files(id);
        self.close_review_commits(id);
        let Some(index) = self.reviews.open.iter().position(|open| open.id == id) else {
            return;
        };
        let open = self.reviews.open.remove(index);
        if let Some(panes) = &open.panes {
            for buffer in panes.buffers {
                self.drop_internal_buffer(buffer);
            }
        }
        self.drop_internal_buffer(open.buffer);
        self.reviews.closed.push((id, open.review));
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

    /// The review whose buffer, or one of whose panes, is `buffer`.
    pub fn review_of(&self, buffer: BufferId) -> Option<ReviewId> {
        Some(self.reviews.holding(buffer)?.id)
    }

    /// What each line of the review buffer of `id` is.
    pub fn review_rows(&self, id: ReviewId) -> Option<&Rows> {
        Some(&self.reviews.get(id)?.rows)
    }

    /// What each line of `buffer` is, a review buffer or a pane of one.
    pub fn review_rows_of(&self, buffer: BufferId) -> Option<&Rows> {
        self.reviews.holding(buffer)?.rows_of(buffer)
    }

    /// Whether a review's new text must wait, as a file read again does
    /// ([`Editor::reload_waits`]): the host keeps it, and the ones after it, until this is
    /// false, and puts them in in order.
    pub fn review_waits(&self) -> bool {
        self.reload_waits()
    }

    /// Puts in the review buffer's whole new text, built on the host's view thread and
    /// compared there with the text it replaces, with what each line is and its highlights.
    /// O(1) and the cursors: every window's cursors and top keep their place by anchor, a line
    /// of a side moved by `moved`, the host's map of where each line of a file went, and a
    /// header's, a note's or a card's row found again (step 4); with no undo. The host calls it
    /// only while [`Editor::review_waits`] is false, keeps the text before at O(1), and frees
    /// it and what this returns off the UI thread.
    pub fn set_review_text(
        &mut self,
        id: ReviewId,
        text: Reloaded,
        rows: Rows,
        highlights: Highlights,
        moved: LineMoves<'_>,
    ) -> ReviewLeftover {
        let Some(buffer_id) = self
            .review_buffer(id)
            .filter(|buffer| self.buffer(*buffer).is_some())
        else {
            return ReviewLeftover::of(rows, highlights, Some(text));
        };
        let kept = (self.reviews.get(id)).map(|open| self.keep_places(buffer_id, &open.rows));
        let new_rows = rows.clone();
        let mut left = self.put_generated_text(buffer_id, text, highlights);
        left.rows = self.put_review_rows(id, rows);
        left.spare = self.take_spare_rows(id);
        if let Some(kept) = kept {
            self.put_places(buffer_id, kept, &new_rows, moved);
            self.follow_buffer_windows(buffer_id);
        }
        left
    }

    /// Scrolls each window showing `buffer` as little as shows its cursor.
    pub(super) fn follow_buffer_windows(&mut self, buffer: BufferId) {
        let shown: Vec<usize> = (0..self.windows.len())
            .filter(|index| self.windows[*index].buffer() == buffer)
            .collect();
        for index in shown {
            self.follow_window(index);
        }
    }

    /// Puts in the whole new text of `buffer_id`, a review buffer, a pane of one, or an internal
    /// buffer showing a side ([`Editor::set_internal_reloaded`]), with its highlights, as
    /// [`Editor::set_review_text`] does, and returns what it replaced or did not take but the
    /// rows, which the caller puts in.
    pub(super) fn put_generated_text(
        &mut self,
        buffer_id: BufferId,
        text: Reloaded,
        highlights: Highlights,
    ) -> ReviewLeftover {
        let Some(buffer) = self.buffer(buffer_id) else {
            return ReviewLeftover {
                highlights: Some(highlights),
                text: Some(Box::new(text)),
                ..ReviewLeftover::none()
            };
        };
        let Reloaded { snapshot, change } = text;
        let old = buffer.text().clone();
        let change = match change {
            None if snapshot.display.invalid_bytes().is_empty()
                && buffer.invalid_bytes().is_empty() =>
            {
                let mut left = ReviewLeftover {
                    text: Some(Box::new(Reloaded { snapshot, change })),
                    ..ReviewLeftover::none()
                };
                left.highlights = self.put_review_highlights(buffer_id, highlights);
                return left;
            }
            None => Change {
                start: 0,
                old_end: old.len(),
                new_end: snapshot.text.len(),
            },
            // One compared with another text than the buffer's, as a pane's made again: whole.
            Some(change) if change.old_end > old.len() || change.start > change.old_end => Change {
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
        let mut left = ReviewLeftover::none();
        if let Some(buffer) = self.buffer_mut(buffer_id) {
            // Taken first, as the reload would drop them here.
            let old_highlights = buffer.take_generated_highlights();
            buffer.replace_with_reload(snapshot);
            left.highlights = match buffer.set_generated_highlights(highlights) {
                Some(refused) => Some(refused),
                None => old_highlights,
            };
        }
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
        let Some(buffer_id) = self
            .review_buffer(id)
            .filter(|buffer| self.paint_covers(*buffer, &rows, &highlights))
        else {
            return Err(ReviewLeftover::of(rows, highlights, None));
        };
        Ok(ReviewLeftover {
            highlights: self.put_review_highlights(buffer_id, highlights),
            rows: self.put_review_rows(id, rows),
            spare: self.take_spare_rows(id),
            ..ReviewLeftover::none()
        })
    }

    /// Whether `rows` cover the lines of `buffer`, a review buffer or a pane, and `highlights`
    /// its text, as a paint for a text that did not change must.
    pub(super) fn paint_covers(
        &self,
        buffer: BufferId,
        rows: &Rows,
        highlights: &Highlights,
    ) -> bool {
        self.buffer(buffer).is_some_and(|shown| {
            let lines = shown.text().max_point().row + 1;
            rows.lines() + 1 >= lines && highlights.len() == shown.text().len()
        })
    }

    /// Puts in a review buffer's highlights, returning the ones before, or these if refused.
    pub(super) fn put_review_highlights(
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

    /// The rows of the review `id`'s panes dropped since, for a leftover to free.
    pub(super) fn take_spare_rows(&mut self, id: ReviewId) -> Vec<Rows> {
        self.reviews
            .get_mut(id)
            .map(|open| std::mem::take(&mut open.spare))
            .unwrap_or_default()
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

    /// The paths of the files the windows showing review `id`'s buffer or a pane of it show
    /// some line of, into `out`, emptied first, each once, in the review's order, for the host
    /// to read them first.
    pub fn shown_review_files(&self, id: ReviewId, out: &mut Vec<Box<[u8]>>) {
        out.clear();
        let Some(open) = self.reviews.get(id) else {
            return;
        };
        for window in &self.windows {
            let Some(rows) = open.rows_of(window.buffer()) else {
                continue;
            };
            let (Some(buffer), Some(regions)) =
                (self.buffer(window.buffer()), self.regions(window.id()))
            else {
                continue;
            };
            let top = buffer
                .display()
                .offset_to_display_point(window.scroll().start)
                .row;
            for line in top..top + regions.text.rows {
                let Some(row) = rows.at(line) else {
                    break;
                };
                if row.kind != RowKind::Filler && out.last().is_none_or(|last| **last != *row.path)
                {
                    out.push(row.path.into());
                }
            }
        }
        out.sort_unstable();
        out.dedup();
    }

    /// Marks the file at `path` of the review `id` as shown at a reading the model does not
    /// hold yet, as when its carry was taken before an earlier reading was applied, or clears
    /// the mark once the model caught up, when the file left the review, or for good when the
    /// review closes. While marked, the comment commands refuse on the file, as a line of the
    /// text shown may not be the model's. Costs a lookup among the files marked.
    pub fn set_review_file_behind(&mut self, id: ReviewId, path: &[u8], behind: bool) {
        let Some(open) = self.reviews.get_mut(id) else {
            return;
        };
        if behind {
            if !open.behind.contains(path) {
                open.behind.insert(path.into());
            }
        } else {
            open.behind.remove(path);
        }
    }

    /// Whether the file at `path` of the review `id` is marked as shown at a reading the model
    /// does not hold yet ([`Editor::set_review_file_behind`]). False for a review not open.
    pub fn review_file_behind(&self, id: ReviewId, path: &[u8]) -> bool {
        self.reviews
            .get(id)
            .is_some_and(|open| open.behind.contains(path))
    }

    /// Says how many of the files listed the review `id`'s running request has read, for its
    /// modeline, or `None` once it read them all.
    pub fn set_review_reading(&mut self, id: ReviewId, reading: Option<(usize, usize)>) {
        if let Some(open) = self.reviews.get_mut(id) {
            open.reading = reading;
        }
    }
}
