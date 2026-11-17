//! A review kept up to date while the agent edits (ADR 0122, "Refresh while the agent edits",
//! step 4): what marks it stale, the refresh that follows once the marks settle, and where each
//! line of its files went, so that its windows keep their places.
//!
//! Must not depend on: key presses or how a frame is drawn; the review's thread looks and reads.
//!
//! - **Stale marks:** a look on the review's thread, asked each [`LOOK_EVERY`] while a window
//!   shows the review and at once when oso gains the focus, which finds a reviewed file, the
//!   listing, the index or `HEAD` moved; a save in oso of a file under the review's root; and an
//!   agent session under it ending a turn.
//! - **Debounced:** a refresh starts [`QUIET`] after the last mark, or [`AT_MOST`] after the
//!   first. One runs at a time: marks during it make one more once it is read.
//! - **Hidden:** a review no window shows makes no looks (Q-E), and a mark only waits until a
//!   window shows it again.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use oso_core::review::{Carried, RowKind, Side};

use crate::app::App;
use crate::review::{Reading, Request};

/// How long the marks must stay quiet before a refresh starts.
const QUIET: Duration = Duration::from_millis(200);
/// How long after the first mark a refresh starts, however the marks go on.
const AT_MOST: Duration = Duration::from_secs(1);
/// How often a shown review looks for changes.
const LOOK_EVERY: Duration = Duration::from_secs(1);

/// What a review keeps of its watching.
#[derive(Debug, Default)]
pub(super) struct Watch {
    /// The first and the last mark since a refresh last started.
    first: Option<Instant>,
    last: Option<Instant>,
    /// Whether a refresh runs, and whether a mark came during it.
    refreshing: bool,
    again: bool,
    /// Whether it was marked stale while no window showed it.
    hidden: bool,
    /// When it next looks, while shown.
    next_look: Option<Instant>,
}

impl Watch {
    /// When something is next due: the refresh, or a look.
    fn deadline(&self) -> Option<Instant> {
        let refresh = self
            .first
            .zip(self.last)
            .map(|(first, last)| (first + AT_MOST).min(last + QUIET));
        refresh.into_iter().chain(self.next_look).min()
    }
}

/// Where line `number` of `side` of the file at `path` went, by the carries `carried` of the
/// text going in: a file carried follows its carry, to the nearest line that stayed for one that
/// went; a file not carried did not change, and its lines stay.
pub(super) fn line_now(
    carried: &[(Arc<Reading>, Carried)],
    path: &[u8],
    side: Side,
    number: u32,
) -> Option<u32> {
    match (carried.iter().rev()).find(|(reading, _)| *reading.entry.path == *path) {
        Some((_, carried)) => carried.line_now(side, number),
        None => Some(number),
    }
}

impl App {
    /// Whether a window shows the review at `index`, as the areas were last looked at.
    fn review_shown(&self, index: usize) -> bool {
        let id = self.reviews.open[index].id;
        self.reviews.areas.iter().any(|area| area.review == id)
    }

    /// Marks the review at `index` stale: a refresh follows once the marks settle, after the
    /// one running, or once a window shows it.
    pub(super) fn mark_review_stale(&mut self, index: usize) {
        let shown = self.review_shown(index);
        let watch = &mut self.reviews.open[index].watch;
        if !shown {
            watch.hidden = true;
        } else if watch.refreshing {
            watch.again = true;
        } else {
            let now = Instant::now();
            watch.first.get_or_insert(now);
            watch.last = Some(now);
        }
    }

    /// A refresh of the review at `index` was asked.
    pub(super) fn review_refreshing(&mut self, index: usize) {
        let watch = &mut self.reviews.open[index].watch;
        watch.refreshing = true;
        watch.first = None;
        watch.last = None;
    }

    /// The refresh of the review at `index` read every file: marks made meanwhile make one more.
    pub(super) fn review_read(&mut self, index: usize) {
        let watch = &mut self.reviews.open[index].watch;
        watch.refreshing = false;
        if std::mem::take(&mut watch.again) {
            self.mark_review_stale(index);
        }
    }

    /// After each event: a review shown again that was marked while hidden refreshes, and one no
    /// longer shown stops looking.
    pub(super) fn sync_review_watches(&mut self) {
        for index in 0..self.reviews.open.len() {
            let shown = self.review_shown(index);
            let watch = &mut self.reviews.open[index].watch;
            match (shown, watch.next_look) {
                (true, None) => watch.next_look = Some(Instant::now() + LOOK_EVERY),
                (false, Some(_)) => watch.next_look = None,
                _ => {}
            }
            if shown && std::mem::take(&mut watch.hidden) {
                self.mark_review_stale(index);
            }
        }
    }

    /// When a review next refreshes or looks.
    pub(in crate::app) fn review_watch_deadline(&self) -> Option<Instant> {
        (self.reviews.open.iter())
            .filter_map(|running| running.watch.deadline())
            .min()
    }

    /// Starts each refresh whose marks settled, and asks each shown review whose time came to
    /// look.
    pub(in crate::app) fn review_watch_due(&mut self) {
        let now = Instant::now();
        for index in 0..self.reviews.open.len() {
            let watch = &mut self.reviews.open[index].watch;
            let settled = (watch.first.zip(watch.last))
                .is_some_and(|(first, last)| now >= (first + AT_MOST).min(last + QUIET));
            let look = watch.next_look.is_some_and(|at| now >= at);
            if look {
                watch.next_look = Some(now + LOOK_EVERY);
            }
            if settled {
                let id = self.reviews.open[index].id;
                self.refresh_review(id);
            } else if look && !self.reviews.open[index].watch.refreshing {
                self.ask_review(index, Request::Look);
            }
        }
    }

    /// oso gained the focus: each shown review looks at once.
    pub(in crate::app) fn reviews_look_now(&mut self) {
        let now = Instant::now();
        for running in &mut self.reviews.open {
            if running.watch.next_look.is_some() {
                running.watch.next_look = Some(now);
            }
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
    /// looked at: no longer said to be changed.
    pub(super) fn see_hunk_at_cursor(&mut self) {
        let (window, shown) = self.editor.tiled();
        let buffer = window.buffer();
        let line = (shown.text())
            .offset_to_point(window.selections.primary().cursor)
            .row;
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
        let seen = (self.editor.review_mut(review)).is_some_and(|model| model.seen(&path, hunk));
        if seen && let Some(index) = self.reviews.index(review) {
            self.tell_cards(index, &path);
        }
    }
}
