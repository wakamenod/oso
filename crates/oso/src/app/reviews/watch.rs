//! A review kept up to date while the agent edits (ADR 0122, "Refresh while the agent edits",
//! step 4): what marks it stale, the refresh that follows once the marks settle, and where each
//! line of its files went, so that its windows keep their places.
//!
//! Must not depend on: key presses or how a frame is drawn; the review's thread looks and reads.
//!
//! - **Stale marks:** a look on the review's thread, asked each [`LOOK_EVERY`] while a window
//!   shows the review and oso's window is not hidden, at once when a window shows it again or
//!   oso gains the focus, which finds a reviewed file, the listing, the index or `HEAD` moved;
//!   a save in oso of a file under the review's root; and an agent session under it ending a
//!   turn.
//! - **Debounced:** a refresh starts [`QUIET`] after the last mark, or [`AT_MOST`] after the
//!   first. One runs at a time: marks during it make one more once it ends, read, stopped or
//!   failed.
//! - **Hidden:** a review no window shows makes no looks (Q-E), and a mark only waits until a
//!   window shows it again. One whose project is no longer trusted takes no mark and makes no
//!   look: the person's `g` starts it over.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use oso_core::review::{Carried, Review, RowKind, Side};

use crate::app::App;
use crate::review::{Reading, Request};

/// How long the marks must stay quiet before a refresh starts.
const QUIET: Duration = Duration::from_millis(200);
/// How long after the first mark a refresh starts, however the marks go on.
const AT_MOST: Duration = Duration::from_secs(1);
/// How often a shown review looks for changes.
const LOOK_EVERY: Duration = Duration::from_secs(1);

/// What a review keeps of its refreshing by itself.
#[derive(Debug, Default)]
pub(super) struct Refresher {
    /// The first and the last mark since a refresh last started.
    first: Option<Instant>,
    last: Option<Instant>,
    /// Whether a refresh runs, and whether a mark came during it.
    refreshing: bool,
    again: bool,
    /// Whether it was marked stale while no window showed it.
    hidden: bool,
    /// When it next looks, while it looks.
    next_look: Option<Instant>,
}

/// What is due for a review.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Due {
    Nothing,
    Refresh,
    Look,
}

impl Refresher {
    /// It is stale as of `now`; `shown`, whether a window shows it.
    fn mark(&mut self, now: Instant, shown: bool) {
        if !shown {
            self.hidden = true;
        } else if self.refreshing {
            self.again = true;
        } else {
            self.first.get_or_insert(now);
            self.last = Some(now);
        }
    }

    /// A refresh was asked.
    fn started(&mut self) {
        self.refreshing = true;
        self.first = None;
        self.last = None;
    }

    /// The refresh ended, read, stopped or failed: a mark made meanwhile makes one more.
    fn ended(&mut self, now: Instant, shown: bool) {
        self.refreshing = false;
        if std::mem::take(&mut self.again) {
            self.mark(now, shown);
        }
    }

    /// After an event: whether a window shows it, and whether it may look. One shown again
    /// looks at once, and refreshes if it was marked while hidden.
    fn showing(&mut self, now: Instant, shown: bool, looks: bool) {
        match (looks, self.next_look) {
            (true, None) => self.next_look = Some(now),
            (false, Some(_)) => self.next_look = None,
            _ => {}
        }
        if shown && std::mem::take(&mut self.hidden) {
            self.mark(now, true);
        }
    }

    /// When something is next due: the refresh, or a look.
    fn deadline(&self) -> Option<Instant> {
        let refresh = (self.first.zip(self.last)).map(|(first, last)| settles(first, last));
        refresh.into_iter().chain(self.next_look).min()
    }

    /// What is due at `now`. Marks that settled are taken, even for a refresh that cannot
    /// start, so that they do not come due again at once; for a review no window shows, they
    /// wait until one does.
    fn due(&mut self, now: Instant, shown: bool) -> Due {
        let settled = (self.first.zip(self.last)).is_some_and(|(a, b)| now >= settles(a, b));
        if settled {
            self.first = None;
            self.last = None;
            if shown {
                return Due::Refresh;
            }
            self.hidden = true;
        }
        if self.next_look.is_some_and(|at| now >= at) {
            self.next_look = Some(now + LOOK_EVERY);
            if !self.refreshing {
                return Due::Look;
            }
        }
        Due::Nothing
    }

    /// Looks at once, if it looks.
    fn look_now(&mut self, now: Instant) {
        if self.next_look.is_some() {
            self.next_look = Some(now);
        }
    }
}

/// When marks first made at `first` and last at `last` settle.
fn settles(first: Instant, last: Instant) -> Instant {
    (first + AT_MOST).min(last + QUIET)
}

/// Where each line of a carried file is in the text going in, by the carries of that text.
pub(super) struct Lines<'a> {
    /// A carry by its file's path; `None` for one made from another reading than the model's,
    /// which the windows do not show.
    by_path: HashMap<&'a [u8], Option<&'a Carried>>,
}

impl<'a> Lines<'a> {
    /// The lines of `carried`, as `model` holds the readings the windows show now. O(files).
    pub(super) fn new(model: Option<&Review>, carried: &'a [(Arc<Reading>, Carried)]) -> Self {
        let mut by_path = HashMap::with_capacity(carried.len());
        for (reading, carry) in carried {
            let path = &*reading.entry.path;
            let now = model.and_then(|model| model.file(path));
            let fresh = match (now, carry.maps_from()) {
                (Some(now), Some(from)) => Arc::ptr_eq(now, from),
                (None, None) => true,
                _ => false,
            };
            by_path.insert(path, fresh.then_some(carry));
        }
        Lines { by_path }
    }

    /// Where line `number` of `side` of the file at `path` went: a file carried follows its
    /// carry, to the nearest line that stayed for one that went; a file not carried did not
    /// change, and its lines stay. `None` when it cannot be said. O(1) for one that stayed.
    pub(super) fn line_now(&self, path: &[u8], side: Side, number: u32) -> Option<u32> {
        match self.by_path.get(path) {
            Some(Some(carried)) => carried.line_now(side, number),
            Some(None) => None,
            None => Some(number),
        }
    }
}

impl App {
    /// Whether a window shows the review at `index`, as the areas were last looked at.
    fn review_shown(&self, index: usize) -> bool {
        let id = self.reviews.open[index].id;
        self.reviews.areas.iter().any(|area| area.review == id)
    }

    /// Whether the review at `index` may refresh by itself: one whose project is no longer
    /// trusted, or whose thread ended, waits for the person's `g`.
    fn review_watched(&self, index: usize) -> bool {
        let running = &self.reviews.open[index];
        !running.revoked
            && running.reviewer.is_some()
            && self.editor.project_trusted(running.project)
    }

    /// Marks the review at `index` stale: a refresh follows once the marks settle, after the
    /// one running, or once a window shows it.
    pub(super) fn mark_review_stale(&mut self, index: usize) {
        if !self.review_watched(index) {
            return;
        }
        let shown = self.review_shown(index);
        self.reviews.open[index]
            .refresher
            .mark(Instant::now(), shown);
    }

    /// A refresh of the review at `index` was asked.
    pub(super) fn refresh_started(&mut self, index: usize) {
        self.reviews.open[index].refresher.started();
    }

    /// The refresh of the review at `index` ended: marks made meanwhile make one more.
    pub(super) fn refresh_ended(&mut self, index: usize) {
        let shown = self.review_shown(index) && self.review_watched(index);
        (self.reviews.open[index].refresher).ended(Instant::now(), shown);
    }

    /// After each event: a review shown again looks at once, and refreshes if it was marked
    /// while hidden; one no longer shown, or while oso's window is hidden, stops looking.
    pub(super) fn sync_review_watches(&mut self) {
        let now = Instant::now();
        for index in 0..self.reviews.open.len() {
            let shown = self.review_shown(index) && self.review_watched(index);
            let looks = shown && !self.reviews.occluded;
            (self.reviews.open[index].refresher).showing(now, shown, looks);
        }
    }

    /// When a review next refreshes or looks.
    pub(in crate::app) fn review_watch_deadline(&self) -> Option<Instant> {
        (self.reviews.open.iter())
            .filter_map(|running| running.refresher.deadline())
            .min()
    }

    /// Starts each refresh whose marks settled, and asks each shown review whose time came to
    /// look.
    pub(in crate::app) fn review_watch_due(&mut self) {
        let now = Instant::now();
        for index in 0..self.reviews.open.len() {
            let shown = self.review_shown(index);
            match self.reviews.open[index].refresher.due(now, shown) {
                Due::Refresh => {
                    let id = self.reviews.open[index].id;
                    self.refresh_review(id, false);
                }
                Due::Look => self.ask_review(index, Request::Look),
                Due::Nothing => {}
            }
        }
    }

    /// oso gained the focus, or its window shows again: each review that looks does so now.
    pub(in crate::app) fn reviews_look_now(&mut self) {
        let now = Instant::now();
        for running in &mut self.reviews.open {
            running.refresher.look_now(now);
        }
    }

    /// oso's window was hidden, or shows again.
    pub(in crate::app) fn reviews_occluded(&mut self, occluded: bool) {
        self.reviews.occluded = occluded;
        self.sync_review_watches();
        if !occluded {
            self.reviews_look_now();
        }
    }

    /// A file at `path` was saved in oso, or an agent working under `path` ended a turn: each
    /// review whose root holds it is stale.
    pub(in crate::app) fn reviews_touched(&mut self, path: &Path) {
        for index in 0..self.reviews.open.len() {
            if path.starts_with(self.reviews.open[index].root.path()) {
                self.mark_review_stale(index);
            }
        }
    }

    /// The hunk the selected window's primary cursor is on, in a review or a pane of one, is
    /// looked at: no longer said to be changed. Only when the person's own input moved the
    /// cursor in the text that was in front of them: a text put in since, or a cursor a refresh
    /// or the agent put there, says nothing was seen yet; nor does a file whose rows the model
    /// lags, whose hunks are numbered otherwise.
    pub(super) fn see_hunk_at_cursor(&mut self) {
        let acted_on = self.reviews.acted_on.take();
        let (window, shown) = self.editor.tiled();
        let buffer = window.buffer();
        let cursor = window.selections.primary().cursor;
        let Some((was_on, revision, was_at)) = acted_on else {
            return;
        };
        if (was_on, revision) != (buffer, shown.revision()) || was_at == cursor {
            return;
        }
        let line = shown.text().offset_to_point(cursor).row;
        let Some(review) = self.editor.review_of(buffer) else {
            return;
        };
        let row = (self.editor.review_rows_of(buffer)).and_then(|rows| {
            let row = rows.at(line)?;
            let hunk = match row.kind {
                RowKind::Hunk { hunk } | RowKind::Lines { hunk, .. } => hunk,
                _ => return None,
            };
            Some((Box::<[u8]>::from(row.path), hunk))
        });
        let Some((path, hunk)) = row else {
            return;
        };
        if self.editor.review_file_behind(review, &path) {
            return;
        }
        let seen = (self.editor.review_mut(review)).is_some_and(|model| model.seen(&path, hunk));
        if seen && let Some(index) = self.reviews.index(review) {
            self.tell_cards(index, &path);
        }
    }
}

#[cfg(test)]
#[path = "watch_tests.rs"]
mod tests;
