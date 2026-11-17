//! How each review shows in the frame (ADR 0122, "The views", step 11): unified, or as its two
//! panes, as the person chose with `|` ([`Editor::review_choice`]). The view thread builds the
//! panes only while the panes are chosen and some area that can hold them shows the review;
//! their texts go in just as the areas switch to them, in one step, as the editor drops panes
//! no window shows after a command.
//!
//! Must not depend on: key presses or how a frame is drawn; this reads the editor's choices and
//! areas and asks it to switch them.
//!
//! - A switch waits while [`Editor::review_waits`], as a review's texts do, so nothing a prompt
//!   or a search acts on changes window under it.
//! - The panes go side by side in an area of [`SIDE_BY_SIDE`] columns or more, and stacked in a
//!   narrower one. [`ReviewChoice::Auto`] shows the review unified until the width's judge.
//! - An area too small for two panes stays unified, said once, until its width changes, and
//!   holds no panes built for it.
//! - Panes the editor dropped while still chosen and shown for are asked for again, once.
//!
//! [`Editor::review_choice`]: oso_core::editor::Editor::review_choice
//! [`Editor::review_waits`]: oso_core::editor::Editor::review_waits

use oso_core::editor::{ReviewArea, ReviewChoice, ReviewLayout};
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
    /// Whether the view was last told to build the panes.
    told: bool,
    /// Whether the panes were asked for again since the editor dropped them.
    asked_again: bool,
    /// Each area too small for two panes, by its window, and the width it refused at.
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
    /// Whether the review at `index` has its panes built: they are chosen, and an area shows the
    /// review that holds them or did not refuse them at its width.
    fn takes_review_panes(&self, index: usize) -> bool {
        let running = &self.reviews.open[index];
        let refused = &running.layout.refused;
        self.editor.review_choice(running.id) == ReviewChoice::Panes
            && (self.reviews.areas.iter())
                .filter(|area| area.review == running.id)
                .any(|area| !refused.contains(&(area.window, area.columns)))
    }

    /// Puts the panes' texts of the review at `index` in and switches its areas to them at
    /// once, if it takes them, else frees them.
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
        self.reviews.open[index].layout.asked_again = false;
        self.sync_review_layouts();
    }

    /// After commands and each event: tells each review's view whether to build the panes, and
    /// switches each area to what its review shows, while nothing holds the windows.
    pub(super) fn sync_review_layouts(&mut self) {
        let mut areas = std::mem::take(&mut self.reviews.areas);
        self.editor.review_areas(&mut areas);
        self.reviews.areas = areas;
        for index in 0..self.reviews.open.len() {
            self.tell_review_panes(index);
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

    /// Tells the view of the review at `index` to build the panes or not, as it takes them, and
    /// asks for them again once when the editor dropped them meanwhile.
    fn tell_review_panes(&mut self, index: usize) {
        let build = self.takes_review_panes(index);
        let dropped = self
            .editor
            .review_panes(self.reviews.open[index].id)
            .is_none();
        let layout = &mut self.reviews.open[index].layout;
        let again = build && layout.told && dropped && !layout.asked_again;
        if build != layout.told || again {
            layout.told = build;
            layout.asked_again = again;
            self.reviews.open[index].view.tell(Tell::Panes(build));
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
        if self.editor.review_choice(area.review) != ReviewChoice::Panes {
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
                // One width a window: the last it refused at.
                refused.retain(|(window, _)| *window != area.window);
                refused.push((area.window, area.columns));
                self.editor.message(format!("No two panes here: {why}"));
                false
            }
        }
    }
}
