//! Highlighting a review's sides from the app (ADR 0122, step 9): each review's view thread
//! gets a handle on the syntax threads, and the reports of its sides go back to it; what the
//! windows show of each review is told to its view when it changes.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here parses, decodes or
//! projects: the view thread does, and the UI thread only hands reports and ranges on.
//!
//! - A view gets a handle only while highlighting is on, and loses it when it turns off, which
//!   closes every document it opened.
//! - A report is handed on in O(1), and the ranges shown are read with two seeks a window.

use std::ops::Range;

use oso_core::buffer::Verdict;
use oso_core::editor::ReviewId;
use oso_core::rope::Rope;
use oso_syntax::{Language, Refused, Report};

use super::super::App;
use super::super::syntax::{Documents, SyntaxKey};
use crate::review::view::{Highlighter, Tell};

/// What the app last told a review's view about its sides.
#[derive(Default)]
pub(in crate::app) struct Told {
    /// Whether it has a highlighter.
    on: bool,
    /// The byte ranges of the review buffer its windows show.
    shown: Vec<Range<usize>>,
}

/// A review's handle on the syntax threads: its sides' keys are its own.
struct Sides {
    documents: Documents<SyntaxKey>,
    review: ReviewId,
}

impl Highlighter for Sides {
    fn open(
        &self,
        key: u64,
        language: Language,
        verdict: Verdict,
        text: Rope,
        shown: Vec<Range<usize>>,
    ) -> Result<(), Refused> {
        let key = SyntaxKey::Side(self.review, key);
        self.documents.open(key, language, verdict, text, shown)
    }

    fn show(&self, key: u64, shown: Vec<Range<usize>>) {
        self.documents
            .show(SyntaxKey::Side(self.review, key), shown);
    }

    fn close(&self, key: u64) {
        self.documents.close(SyntaxKey::Side(self.review, key));
    }
}

impl App {
    /// Hands each review's view a highlighter while highlighting is on, or takes it back, and
    /// tells it what its windows show when that changed.
    pub(super) fn sync_review_sides(&mut self) {
        let open = &self.reviews.open;
        self.reviews
            .sides
            .retain(|id, _| open.iter().any(|running| running.id == *id));
        if open.is_empty() {
            return;
        }
        let on = self.editor.font_lock();
        let mut ranges = std::mem::take(&mut self.reviews.ranges);
        self.editor.shown_ranges(&mut ranges);
        for index in 0..self.reviews.open.len() {
            let id = self.reviews.open[index].id;
            if on != self.reviews.sides.get(&id).is_some_and(|told| told.on) {
                let documents = if on { self.side_documents() } else { None };
                let highlighter = documents.map(|documents| {
                    Box::new(Sides {
                        documents,
                        review: id,
                    }) as Box<dyn Highlighter>
                });
                let told = self.reviews.sides.entry(id).or_default();
                told.on = highlighter.is_some();
                told.shown.clear();
                let running = &self.reviews.open[index];
                let top = running.root.path().to_path_buf();
                running.view.tell(Tell::Highlighter { highlighter, top });
            }
            let Some(told) = self.reviews.sides.get_mut(&id).filter(|told| told.on) else {
                continue;
            };
            let buffer = self.editor.review_buffer(id);
            let shown = (ranges.iter())
                .filter(|(shown, _)| Some(*shown) == buffer)
                .map(|(_, range)| range.clone());
            if !shown.clone().eq(told.shown.iter().cloned()) {
                told.shown = shown.collect();
                self.reviews.open[index]
                    .view
                    .tell(Tell::Shown(told.shown.clone()));
            }
        }
        self.reviews.ranges = ranges;
    }

    /// Hands a report of a review's side to its view; one of a review closed since is dropped.
    pub(in crate::app) fn review_side_reported(&mut self, review: ReviewId, report: Report<u64>) {
        if let Some(index) = self.reviews.index(review) {
            self.reviews.open[index].view.tell(Tell::Syntax(report));
        }
    }
}
