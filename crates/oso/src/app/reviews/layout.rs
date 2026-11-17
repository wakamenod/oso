//! How each review shows in the frame (ADR 0122, "The views", step 11): unified, or as its two
//! panes, as the person chose with `|`. The view thread builds the panes only while an area of
//! the frame shows the review and the panes are chosen; their first texts go in just before
//! the areas switch to them, as a command drops panes no window shows.
//!
//! Must not depend on: key presses or how a frame is drawn; this reads the editor's areas and
//! asks it to switch them.
//!
//! - A switch waits while [`Editor::review_waits`](oso_core::editor::Editor::review_waits), as a
//!   review's texts do, so nothing a prompt or a search acts on changes window under it.
//! - The panes go side by side in an area of [`SIDE_BY_SIDE`] columns or more, and stacked in a
//!   narrower one.
//! - An area too small for two panes stays unified, said once, until its width changes.

use oso_core::editor::{ReviewArea, ReviewId, ReviewLayout};
use oso_core::frame::Way;
use oso_core::window::WindowId;

use crate::app::App;
use crate::review::view::{PaneText, Tell};

/// The width in cells, gutters and dividers too, from which a review's two panes go side by
/// side (ADR 0122, Q6).
pub(super) const SIDE_BY_SIDE: usize = 160;

/// What an open review keeps of how it shows.
#[derive(Debug, Default)]
pub(super) struct Layout {
    /// Whether the person chose two panes.
    panes: bool,
    /// Whether the view was last told to build the panes.
    told: bool,
    /// The areas too small for two panes, by window and width, so that each is said once.
    refused: Vec<(WindowId, usize)>,
}

/// The way the panes go in an area `columns` wide.
fn way_for(columns: usize) -> Way {
    if columns >= SIDE_BY_SIDE {
        Way::Across
    } else {
        Way::Down
    }
}

impl App {
    /// Takes the person's choice for the review `id`: two panes, or unified. The automatic
    /// layout, `None`, comes with the width's judge.
    pub(super) fn choose_review_layout(&mut self, id: ReviewId, panes: Option<bool>) {
        let Some(index) = self.reviews.index(id) else {
            return;
        };
        let layout = &mut self.reviews.open[index].layout;
        layout.panes = panes.unwrap_or(false);
        layout.refused.clear();
        self.sync_review_layouts();
    }

    /// Whether the review at `index` takes the panes' texts now: they are chosen and some area
    /// shows the review.
    pub(super) fn takes_review_panes(&self, index: usize) -> bool {
        let running = &self.reviews.open[index];
        running.layout.panes
            && self
                .reviews
                .areas
                .iter()
                .any(|area| area.review == running.id)
    }

    /// Puts the panes' texts of the review at `index` in, if it takes them, else frees them.
    pub(super) fn put_in_review_panes(&mut self, index: usize, panes: Box<[PaneText; 2]>) {
        if !self.takes_review_panes(index) {
            return self.freer.free(panes);
        }
        let id = self.reviews.open[index].id;
        let before: Vec<_> = (self.editor.review_panes(id).into_iter())
            .flat_map(|(old, new)| [old, new])
            .filter_map(|buffer| self.editor.buffer(buffer))
            .map(|shown| (shown.text().clone(), shown.display()))
            .collect();
        let [old, new] = *panes;
        let left = self.editor.set_review_panes(id, old, new);
        self.freer.free((before, left));
    }

    /// After commands and each event: tells each review's view whether to build the panes, and
    /// switches each area to what its review shows, while nothing holds the windows.
    pub(super) fn sync_review_layouts(&mut self) {
        let mut areas = std::mem::take(&mut self.reviews.areas);
        self.editor.review_areas(&mut areas);
        self.reviews.areas = areas;
        for index in 0..self.reviews.open.len() {
            let build = self.takes_review_panes(index);
            let running = &mut self.reviews.open[index];
            if build != running.layout.told {
                running.layout.told = build;
                running.view.tell(Tell::Panes(build));
            }
        }
        if self.editor.review_waits() {
            return;
        }
        let areas = std::mem::take(&mut self.reviews.areas);
        let mut switched = false;
        for area in &areas {
            switched |= self.switch_review_area(area);
        }
        self.reviews.areas = areas;
        if switched {
            self.request_redraw();
        }
    }

    /// Switches `area` to what its review shows. Returns whether it changed.
    fn switch_review_area(&mut self, area: &ReviewArea) -> bool {
        let Some(index) = self.reviews.index(area.review) else {
            return false;
        };
        let shown = self
            .editor
            .window_by_id(area.window)
            .map(|window| window.buffer());
        let panes = self.editor.review_panes(area.review);
        let in_pane = panes.is_some_and(|(old, new)| shown == Some(old) || shown == Some(new));
        if !self.reviews.open[index].layout.panes {
            return in_pane && self.editor.show_review_unified(area.window);
        }
        if area.layout != ReviewLayout::Unified || panes.is_none() {
            return false;
        }
        let refused = &mut self.reviews.open[index].layout.refused;
        if refused.contains(&(area.window, area.columns)) {
            return false;
        }
        match self
            .editor
            .show_review_panes(area.window, way_for(area.columns))
        {
            Ok(()) => true,
            Err(why) => {
                refused.push((area.window, area.columns));
                self.editor.message(format!("No two panes here: {why}"));
                false
            }
        }
    }
}
