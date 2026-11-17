//! The width's judge (ADR 0122, "The views", Q6): what each area a review shows in wants, from
//! its width and the person's choice, judged only when the area first shows, when the choice
//! changes, and when its width changed and then stayed the same for [`SETTLE`], so that
//! dragging a frame's edge switches at most once. Never while scrolling, moving or refreshing,
//! which change no width.
//!
//! Must not depend on: the editor, the frame or the clock; the caller says what the areas are
//! and when it is.
//!
//! - Automatic: two panes side by side from [`SIDE_BY_SIDE`] columns, unified below
//!   [`UNIFIED_BELOW`], and between the two what the area shows already.
//! - Two panes chosen: side by side from [`SIDE_BY_SIDE`] columns, stacked below
//!   [`UNIFIED_BELOW`], and between the two the way they go already.
//! - Unified chosen: unified.

use std::time::{Duration, Instant};

use oso_core::editor::{ReviewChoice, ReviewId, ReviewLayout};
use oso_core::frame::Way;
use oso_core::window::WindowId;

/// The width in cells, gutters and dividers too, from which a review's two panes go side by
/// side (ADR 0122, Q6, Q-E).
pub(super) const SIDE_BY_SIDE: usize = 160;
/// The width below which an automatic review shows unified again, and two panes chosen go
/// stacked.
pub(super) const UNIFIED_BELOW: usize = 150;
/// How long an area's new width must stay the same before it is judged.
pub(super) const SETTLE: Duration = Duration::from_millis(100);

/// One area's width as last seen, since when, and what was judged for it.
#[derive(Clone, Copy, Debug)]
struct Seen {
    window: WindowId,
    /// The review it shows: a window showing another review is another area.
    review: ReviewId,
    columns: usize,
    since: Instant,
    /// The width and the choice it was judged at, and what it wants, once judged.
    judged: Option<(usize, ReviewChoice, ReviewLayout)>,
}

/// What each area a review shows in wants, by its window and review.
#[derive(Debug, Default)]
pub(super) struct Judge {
    seen: Vec<Seen>,
}

/// The way two panes go in an area `columns` wide that shows `shown`.
fn way(columns: usize, shown: ReviewLayout) -> Way {
    match shown {
        _ if columns >= SIDE_BY_SIDE => Way::Across,
        ReviewLayout::Panes(way) if columns >= UNIFIED_BELOW => way,
        _ => Way::Down,
    }
}

/// What an area `columns` wide showing `shown` wants under `choice`.
fn wanted(choice: ReviewChoice, columns: usize, shown: ReviewLayout) -> ReviewLayout {
    match choice {
        ReviewChoice::Unified => ReviewLayout::Unified,
        ReviewChoice::Panes => ReviewLayout::Panes(way(columns, shown)),
        ReviewChoice::Auto => match shown {
            ReviewLayout::Panes(_) if columns < UNIFIED_BELOW => ReviewLayout::Unified,
            ReviewLayout::Panes(_) => ReviewLayout::Panes(way(columns, shown)),
            ReviewLayout::Unified if columns >= SIDE_BY_SIDE => ReviewLayout::Panes(Way::Across),
            ReviewLayout::Unified => ReviewLayout::Unified,
        },
    }
}

impl Judge {
    /// Takes the areas as they are at `now`, each by its window, its review and its width: a
    /// width that changed starts settling, and areas gone are forgotten, as is one whose window
    /// shows another review now.
    pub(super) fn see(
        &mut self,
        areas: impl IntoIterator<Item = (WindowId, ReviewId, usize)>,
        now: Instant,
    ) {
        let mut seen = Vec::with_capacity(self.seen.len());
        for (window, review, columns) in areas {
            let before = (self.seen.iter())
                .find(|seen| seen.window == window && seen.review == review)
                .copied();
            seen.push(match before {
                Some(before) if before.columns == columns => before,
                Some(before) => Seen {
                    columns,
                    since: now,
                    ..before
                },
                None => Seen {
                    window,
                    review,
                    columns,
                    since: now,
                    judged: None,
                },
            });
        }
        self.seen = seen;
    }

    /// What the area of `window` wants under `choice` at `now`, showing `shown`: judged again
    /// when it was never judged, when the choice changed, or when its width changed and has
    /// settled; else what was judged last. `None` for an area not seen.
    pub(super) fn want(
        &mut self,
        window: WindowId,
        choice: ReviewChoice,
        shown: ReviewLayout,
        now: Instant,
    ) -> Option<ReviewLayout> {
        let seen = self.seen.iter_mut().find(|seen| seen.window == window)?;
        let due = match seen.judged {
            None => true,
            Some((_, judged, _)) if judged != choice => true,
            Some((columns, ..)) => columns != seen.columns && now >= seen.since + SETTLE,
        };
        if due {
            let want = wanted(choice, seen.columns, shown);
            seen.judged = Some((seen.columns, choice, want));
        }
        seen.judged.map(|(.., want)| want)
    }

    /// What the area of `window` wanted when last judged, and the width it was judged at,
    /// judging nothing.
    pub(super) fn wanted(&self, window: WindowId) -> Option<(ReviewLayout, usize)> {
        let seen = self.seen.iter().find(|seen| seen.window == window)?;
        seen.judged.map(|(columns, _, want)| (want, columns))
    }

    /// When the next width settles, to be judged then, if one is settling.
    pub(super) fn deadline(&self) -> Option<Instant> {
        self.seen
            .iter()
            .filter(|seen| {
                seen.judged
                    .is_some_and(|(columns, ..)| columns != seen.columns)
            })
            .map(|seen| seen.since + SETTLE)
            .min()
    }
}

#[cfg(test)]
#[path = "judge_tests.rs"]
mod tests;
