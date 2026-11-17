//! A review's files pane and filter in the app (ADR 0122, step 6b): tells each review's view
//! the filter the editor holds and the files pane's width while a window shows the pane, and
//! puts the pane's texts in.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here lays out a text: the
//! view thread does, and its text goes in at O(1), what it replaces freed off the UI thread.

use oso_core::buffer::BufferId;
use oso_core::review::Filter;

use crate::app::App;
use crate::review::view::{PaneText, Tell};

/// What a review's view was last told of its files pane and filter.
#[derive(Debug, Default)]
pub(super) struct Told {
    filter: Option<Filter>,
    width: Option<usize>,
    /// The pane's buffer: a new one, as when the pane was hidden and shown again since, takes
    /// its text whole.
    pane: Option<BufferId>,
}

impl App {
    /// After commands and each event: tells each review's view its filter and its files pane's
    /// width, when either changed. Costs a comparison and a look at the pane's window a review.
    pub(super) fn sync_review_files(&mut self) {
        for index in 0..self.reviews.open.len() {
            let id = self.reviews.open[index].id;
            let filter = self.editor.review_filter(id).cloned();
            let pane = self.editor.review_files(id);
            let width = pane
                .and_then(|buffer| {
                    self.editor
                        .windows()
                        .find(|window| window.buffer() == buffer)
                })
                .and_then(|window| self.editor.frame().regions(window.id()))
                .map(|regions| regions.text.columns);
            let running = &mut self.reviews.open[index];
            if running.files.filter != filter {
                running.files.filter.clone_from(&filter);
                running.view.tell(Tell::Filter(filter));
            }
            if running.files.pane != pane && running.files.width.is_some() {
                // The text built for the pane before goes to no pane.
                running.files.width = None;
                running.view.tell(Tell::Files(None));
            }
            running.files.pane = pane;
            if running.files.width != width {
                running.files.width = width;
                running.view.tell(Tell::Files(width));
            }
        }
    }

    /// Puts in the files pane's text the view of the review at `index` built, while it has a
    /// pane; what it replaces, or the text with no pane, is freed off the UI thread.
    pub(super) fn put_in_review_files(&mut self, index: usize, files: Box<PaneText>) {
        let id = self.reviews.open[index].id;
        let Some(buffer) = self.editor.review_files(id) else {
            return self.freer.free(files);
        };
        let before =
            (self.editor.buffer(buffer)).map(|shown| (shown.text().clone(), shown.display()));
        let (text, rows, highlights) = *files;
        let left = self.editor.set_review_files(id, text, rows, highlights);
        self.freer.free((before, left));
    }
}
