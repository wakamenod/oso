//! The open reviews (ADR 0122, "The views"): each one's model, its review buffer and the rows
//! that say what each of its lines is, what commands ask of the host for them, and what the
//! review menu shows of each project's scopes (ADR 0117, "Live content").
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
//! - Backgrounds are cells worked out per frame for the rows in view only.

use std::ops::Range;

use super::Editor;
use crate::buffer::{BufferId, Mode, Rewrap};
use crate::frame::{self, CellRect};
use crate::load::{Change, Reloaded};
use crate::project::ProjectId;
use crate::review::{Review, RowKind, Rows, Side};
use crate::sum_tree::Bias;
use crate::syntax::Highlights;
use crate::window::{Screen, WindowId};

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
    fn index(self) -> usize {
        self as usize
    }
}

/// How many files a scope differs in, as the menu shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Count {
    /// Being counted.
    Running,
    Files(usize),
    /// More than a review lists.
    More,
    /// The count could not be made.
    Failed,
}

/// What the review menu shows of one project: each scope's count from the menu's latest
/// opening, the last scope reviewed, when the session's baseline was taken, and the branch's
/// base.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScopeMenu {
    counts: [Option<Count>; 8],
    last: Option<ScopeKind>,
    /// Seconds since the Unix epoch.
    baseline: Option<u64>,
    base: Option<String>,
    generation: u64,
}

impl ScopeMenu {
    /// The count of `scope` from this opening, `None` if none was asked for.
    pub fn count(&self, scope: ScopeKind) -> Option<Count> {
        self.counts[scope.index()]
    }

    /// The scope last reviewed in the project.
    pub fn last(&self) -> Option<ScopeKind> {
        self.last
    }

    /// When the session's baseline was taken, in seconds since the Unix epoch.
    pub fn baseline_taken(&self) -> Option<u64> {
        self.baseline
    }

    /// The base the branch scope compares with, once the host found it.
    pub fn base(&self) -> Option<&str> {
        self.base.as_deref()
    }

    /// The opening the counts are of.
    pub fn generation(&self) -> u64 {
        self.generation
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

/// The backgrounds a review window's rows in view take, for a frame, kept from frame to frame
/// so that a frame allocates nothing once they have grown.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReviewCells {
    pub added: Vec<CellRect>,
    pub removed: Vec<CellRect>,
    pub refine_added: Vec<CellRect>,
    pub refine_removed: Vec<CellRect>,
    pub headers: Vec<CellRect>,
    pub cards: Vec<CellRect>,
}

impl ReviewCells {
    /// Empties every list, keeping their room.
    pub fn clear(&mut self) {
        self.added.clear();
        self.removed.clear();
        self.refine_added.clear();
        self.refine_removed.clear();
        self.headers.clear();
        self.cards.clear();
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

    fn menu_mut(&mut self, project: ProjectId) -> &mut ScopeMenu {
        let index = match self.menus.iter().position(|(id, _)| *id == project) {
            Some(index) => index,
            None => {
                self.menus.push((project, ScopeMenu::default()));
                self.menus.len() - 1
            }
        };
        &mut self.menus[index].1
    }
}

/// Which background a row of `kind` takes.
fn band(kind: RowKind) -> Band {
    match kind {
        RowKind::Lines {
            side: Side::Old, ..
        } => Band::Removed,
        RowKind::Lines {
            side: Side::New, ..
        } => Band::Added,
        RowKind::File | RowKind::Hunk { .. } => Band::Header,
        RowKind::Label { .. } | RowKind::Card { .. } => Band::Card,
        RowKind::Note => Band::None,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Band {
    Added,
    Removed,
    Header,
    Card,
    None,
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
    /// host calls it only while [`Editor::review_waits`] is false, and frees the text before
    /// off the UI thread.
    pub fn set_review_text(
        &mut self,
        id: ReviewId,
        text: Reloaded,
        rows: Rows,
        highlights: Highlights,
    ) {
        let Some(open) = self.reviews.get_mut(id) else {
            return;
        };
        open.rows = rows;
        let buffer_id = open.buffer;
        let Some(buffer) = self.buffer(buffer_id) else {
            return;
        };
        let Reloaded { snapshot, change } = text;
        let old = buffer.text().clone();
        let change = match change {
            None if snapshot.display.invalid_bytes().is_empty()
                && buffer.invalid_bytes().is_empty() =>
            {
                if let Some(buffer) = self.buffer_mut(buffer_id) {
                    buffer.set_generated_highlights(highlights);
                }
                return;
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
        if let Some(buffer) = self.buffer_mut(buffer_id) {
            buffer.replace_with_reload(snapshot);
            buffer.set_generated_highlights(highlights);
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
    }

    /// Puts in what each line is and the highlights for a text that did not change, as when
    /// refinement found more to mark. False, changing nothing, when the rows do not cover the
    /// buffer's lines or the highlights its text.
    pub fn set_review_paint(&mut self, id: ReviewId, rows: Rows, highlights: Highlights) -> bool {
        let Some(buffer_id) = self.review_buffer(id) else {
            return false;
        };
        let Some(buffer) = self.buffer(buffer_id) else {
            return false;
        };
        let lines = buffer.text().max_point().row + 1;
        if rows.lines() + 1 < lines || highlights.len() != buffer.text().len() {
            return false;
        }
        if let Some(buffer) = self.buffer_mut(buffer_id) {
            buffer.set_generated_highlights(highlights);
        }
        if let Some(open) = self.reviews.get_mut(id) {
            open.rows = rows;
        }
        true
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

    /// The backgrounds of the review rows the window `id` shows, as `screen` shows it
    /// ([`Editor::screen_of`]), into `out`, emptied first: none for a window that shows no
    /// review buffer. Only the rows in view are looked at.
    pub fn review_cells(&self, window: WindowId, screen: &Screen, out: &mut ReviewCells) {
        out.clear();
        let Some((shown_window, shown, regions)) = self.window_in_tree(window) else {
            return;
        };
        let Some(review) = self.review_of(shown_window.buffer()) else {
            return;
        };
        let Some(rows) = self.review_rows(review) else {
            return;
        };
        let area = regions.text;
        let display = shown.display();
        let text = shown.text();
        let mut spans: Vec<Range<usize>> = Vec::new();
        for (index, screen_row) in screen.rows().iter().enumerate().take(area.rows) {
            let Some(row) = rows.at(screen_row.line) else {
                continue;
            };
            let kind = band(row.kind);
            let list = match kind {
                Band::Added => &mut out.added,
                Band::Removed => &mut out.removed,
                Band::Header => &mut out.headers,
                Band::Card => &mut out.cards,
                Band::None => continue,
            };
            let at = area.row + index;
            match list.last_mut() {
                Some(last) if last.row + last.rows == at => last.rows += 1,
                _ => list.push(CellRect {
                    row: at,
                    column: area.column,
                    rows: 1,
                    columns: area.columns,
                }),
            }
            let refined = rows.refined(screen_row.line);
            if refined.is_empty() || !matches!(kind, Band::Added | Band::Removed) {
                continue;
            }
            let start = text.line_start(screen_row.line);
            let end = text.line_end(screen_row.line);
            spans.clear();
            spans.extend(refined.iter().filter_map(|bytes| {
                let from = (start + bytes.start as usize).min(end);
                let to = (start + bytes.end as usize).min(end);
                let from = text.clip_offset(from, Bias::Left);
                let to = text.clip_offset(to, Bias::Right);
                (from < to).then_some(from..to)
            }));
            let list = if kind == Band::Added {
                &mut out.refine_added
            } else {
                &mut out.refine_removed
            };
            for span in &spans {
                frame::match_cells_on_row(area, (screen, index), &display, span, list);
            }
        }
    }

    /// What the review menu shows of `project`, once it opened for it.
    pub fn scope_menu(&self, project: ProjectId) -> Option<&ScopeMenu> {
        let (_, menu) = self.reviews.menus.iter().find(|(id, _)| *id == project)?;
        Some(menu)
    }

    /// Starts a new opening of the review menu for `project`: every count is forgotten, and
    /// those of `scopes` are running. Returns the opening's generation, which the counts that
    /// come back name; counts of an earlier one are dropped.
    pub fn start_scope_counts(&mut self, project: ProjectId, scopes: &[ScopeKind]) -> u64 {
        let menu = self.reviews.menu_mut(project);
        menu.generation += 1;
        menu.counts = [None; 8];
        for scope in scopes {
            menu.counts[scope.index()] = Some(Count::Running);
        }
        menu.generation
    }

    /// Takes the count of `scope` in `project` from the opening `generation`. False, changing
    /// nothing, for an earlier opening's.
    pub fn set_scope_count(
        &mut self,
        project: ProjectId,
        generation: u64,
        scope: ScopeKind,
        count: Count,
    ) -> bool {
        let menu = self.reviews.menu_mut(project);
        if menu.generation != generation {
            return false;
        }
        menu.counts[scope.index()] = Some(count);
        true
    }

    /// Takes the base the branch scope compares with in `project`, found for the opening
    /// `generation`; an earlier opening's is dropped.
    pub fn set_branch_base(&mut self, project: ProjectId, generation: u64, base: Option<String>) {
        let menu = self.reviews.menu_mut(project);
        if menu.generation == generation {
            menu.base = base;
        }
    }

    /// Says when the session's baseline of `project` was taken, in seconds since the Unix
    /// epoch, or that it has none.
    pub fn set_baseline_taken(&mut self, project: ProjectId, at: Option<u64>) {
        self.reviews.menu_mut(project).baseline = at;
    }

    /// Remembers `scope` as the last one reviewed in `project`.
    pub fn set_last_scope(&mut self, project: ProjectId, scope: ScopeKind) {
        self.reviews.menu_mut(project).last = Some(scope);
    }
}
