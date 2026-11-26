//! The review the person left last (ADR 0184), which `review-layout` continues where no
//! layout is in front: when each review was left and in which space, among the reviews open,
//! one closed in this run and those saved.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here reads a file: the
//! saved reviews are those the store listed, and nothing here runs git.
//!
//! - A review is left as the person opens it, and as it stops being what the selected window
//!   shows: `q`, `C-x k`, the layout's restore, a space switch or another buffer. The one in
//!   front is left as oso goes or restarts, and as `review-layout` continues. One a restart or
//!   a step opens keeps the stamp it was saved with.
//! - The stamp is in memory at once and in the review's saved header at its next save, which
//!   a stamp alone makes due.

use std::path::PathBuf;

use oso_core::editor::ReviewId;

use super::store;
use crate::app::App;
use crate::review::Scope;
use crate::review::saved::{self, Header, Place};

/// When a review closed in this run was left, and what opens it again.
#[derive(Clone, Debug)]
pub(in crate::app::reviews) struct Left {
    /// Its saved file's name.
    name: String,
    root: PathBuf,
    scope: Scope,
    pull: Option<u64>,
    space: Option<String>,
    at: u64,
    opened: u64,
}

/// The review the selected window showed at the last event, in the space in front then.
#[derive(Clone, Debug)]
pub(in crate::app::reviews) struct Front {
    id: ReviewId,
    space: Option<String>,
    /// The selected window's buffer, cursor, view start and buffer revision its place was
    /// taken at, so that it is taken again only once one moved.
    seen: (oso_core::buffer::BufferId, usize, usize, u64),
}

/// What `review-layout` continues.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app::reviews) enum Continue {
    /// A review open.
    Open(ReviewId),
    /// A review closed, or from before oso started: opened from its saved file.
    Saved {
        root: PathBuf,
        scope: Scope,
        /// The pull request a commit's review was stepped to from (ADR 0167).
        pull: Option<u64>,
    },
}

/// One review that may be continued: when it was left, when it was opened, and in which
/// space.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app::reviews) struct Candidate {
    pub(in crate::app::reviews) left: u64,
    pub(in crate::app::reviews) opened: u64,
    pub(in crate::app::reviews) space: Option<String>,
    pub(in crate::app::reviews) what: Continue,
}

/// The review left last among `candidates`: the greatest stamp, then opened last; of two the
/// same, the first given, so that an open review goes before a saved one. A saved review of
/// no stamp, from before ADR 0184, counts as left at 0.
pub(in crate::app::reviews) fn last_left(
    candidates: impl Iterator<Item = Candidate>,
) -> Option<Candidate> {
    candidates.fold(None, |best: Option<Candidate>, next| match best {
        Some(best) if (next.left, next.opened) <= (best.left, best.opened) => Some(best),
        _ => Some(next),
    })
}

/// The candidates of the saved reviews `listed` but those named in `skip`, as open or
/// closed in this run, whose own stamps are newer.
pub(in crate::app::reviews) fn listed_candidates<'a>(
    listed: &'a [(String, String, Header)],
    skip: &'a [String],
) -> impl Iterator<Item = Candidate> + 'a {
    (listed.iter())
        .filter(move |(_, name, _)| !skip.contains(name))
        .map(|(_, _, header)| Candidate {
            left: header.left.unwrap_or(0),
            opened: header.opened,
            space: header.space.clone(),
            what: Continue::Saved {
                root: header.root.clone(),
                scope: header.scope.clone(),
                pull: header.pull,
            },
        })
}

impl App {
    /// The id of the space in front, as the spaces record names it.
    pub(in crate::app::reviews) fn front_space_text(&self) -> Option<String> {
        self.front_space_id().map(|id| id.as_str().to_owned())
    }

    /// The review at `index` was left now, in `space`.
    pub(in crate::app::reviews) fn note_left(&mut self, index: usize, space: Option<String>) {
        let saving = &mut self.reviews.open[index].saving;
        saving.left = store::now().max(saving.left);
        saving.space = space;
    }

    /// After each event: the review the selected window showed at the last one is left once
    /// it shows another thing, and the place of the one it shows now is taken once it moved.
    /// Costs a look at the selected window, then O(log of rows) once it moved.
    pub(in crate::app::reviews) fn sync_review_front(&mut self) {
        let (window, _) = self.editor.tiled();
        let buffer = window.buffer();
        let now = (super::super::slot::owner(&self.editor, buffer))
            .filter(|id| self.reviews.index(*id).is_some());
        let before = self.reviews.keep.front.take();
        if let Some(before) = &before
            && Some(before.id) != now
            && let Some(index) = self.reviews.index(before.id)
        {
            self.note_left(index, before.space.clone());
        }
        let Some(id) = now else {
            return;
        };
        let (window, shown) = self.editor.tiled();
        let seen = (
            buffer,
            window.selections.primary().cursor,
            window.scroll().start,
            shown.revision(),
        );
        let space = self.front_space_text();
        let moved = before.is_none_or(|before| before.id != id || before.seen != seen);
        if moved && let Some(index) = self.reviews.index(id) {
            let place = self.review_place(id);
            if place.is_some() {
                self.reviews.open[index].saving.place_seen = place;
            }
        }
        self.reviews.keep.front = Some(Front { id, space, seen });
    }

    /// The review `id` closed: left now if it was in front, and kept as the one closed left
    /// last if it was.
    pub(in crate::app::reviews) fn closed_left(&mut self, id: ReviewId) {
        let Some(index) = self.reviews.index(id) else {
            return;
        };
        if let Some(front) = &self.reviews.keep.front
            && front.id == id
        {
            let space = front.space.clone();
            self.reviews.keep.front = None;
            self.note_left(index, space);
        }
        let running = &self.reviews.open[index];
        let saving = &running.saving;
        let root = running.root.path().to_owned();
        let newer = (self.reviews.keep.left.as_ref()).is_none_or(|left| saving.left >= left.at);
        if newer && root.to_str().is_some() {
            self.reviews.keep.left = Some(Left {
                name: saved::file_name(&root, &running.scope),
                root,
                scope: running.scope.clone(),
                pull: self.pull_number(running),
                space: saving.space.clone(),
                at: saving.left,
                opened: saving.opened,
            });
        }
    }

    /// The review in front is left now: as oso goes or restarts, and as `review-layout`
    /// continues, so that the review shown where it is pressed is the one laid out.
    pub(in crate::app::reviews) fn note_front_left(&mut self) {
        if let Some(front) = self.reviews.keep.front.clone()
            && let Some(index) = self.reviews.index(front.id)
        {
            self.note_left(index, front.space);
        }
    }

    /// The review `review-layout` continues: the one left last of those open, the one closed
    /// left last in this run and those saved; none while nothing was left or saved.
    pub(in crate::app::reviews) fn review_to_continue(&self) -> Option<Candidate> {
        let mut skip: Vec<String> = (self.reviews.open.iter())
            .filter_map(|running| {
                let root = running.root.path();
                root.to_str()
                    .map(|_| saved::file_name(root, &running.scope))
            })
            .collect();
        let open = (self.reviews.open.iter()).map(|running| Candidate {
            left: running.saving.left,
            opened: running.saving.opened,
            space: running.saving.space.clone(),
            what: Continue::Open(running.id),
        });
        let closed = (self.reviews.keep.left.as_ref())
            .filter(|left| !skip.contains(&left.name))
            .map(|left| Candidate {
                left: left.at,
                opened: left.opened,
                space: left.space.clone(),
                what: Continue::Saved {
                    root: left.root.clone(),
                    scope: left.scope.clone(),
                    pull: left.pull,
                },
            });
        if let Some(left) = &self.reviews.keep.left {
            skip.push(left.name.clone());
        }
        let candidates: Vec<Candidate> = open
            .chain(closed)
            .chain(listed_candidates(&self.reviews.keep.listed, &skip))
            .collect();
        last_left(candidates.into_iter())
    }

    /// Where the cursor of the review at `index` was last seen, to put back as it continues.
    pub(in crate::app::reviews) fn place_seen(&self, index: usize) -> Option<Place> {
        let saving = &self.reviews.open[index].saving;
        saving
            .place_seen
            .clone()
            .or_else(|| saving.place_handed.clone())
    }
}

#[cfg(test)]
#[path = "left_tests.rs"]
mod tests;
