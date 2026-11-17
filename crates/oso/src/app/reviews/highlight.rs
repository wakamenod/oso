//! Highlighting a review's sides from the app (ADR 0122, step 9): each review's view thread
//! gets a handle on the syntax threads, and the reports of its sides go back to it; what the
//! windows show of each review is told to its view when it changes.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here parses, decodes or
//! projects: the view thread does, and the UI thread only hands reports and ranges on.
//!
//! - A view gets a handle only while highlighting is on, and loses it when it turns off, which
//!   closes every document it opened.
//! - Each open buffer under a review's top is handed to its view, as an O(1) copy of its text
//!   and highlights, after each report it takes and when highlighting turns on, so that a side
//!   whose text it is takes its runs. Never a buffer that may hold secrets.
//! - A report is handed on in O(1), and the ranges shown are read with two seeks a window.

use std::ops::Range;
use std::path::{Component, Path};

use oso_core::buffer::{BufferId, Verdict};
use oso_core::editor::ReviewId;
use oso_core::rope::Rope;
use oso_syntax::{Language, Refused, Report};

use super::super::App;
use super::super::syntax::{Documents, SyntaxKey};
use crate::review::view::{BufferSide, Highlighter, Tell, View};

/// What the app last told a review's view about its sides.
#[derive(Default)]
pub(in crate::app) struct Told {
    /// Whether it has a highlighter.
    on: bool,
    /// The byte ranges of the review buffer its windows show.
    shown: Vec<Range<usize>>,
}

/// A review's handle on the syntax threads: its sides' keys are its own.
struct SideHighlighter {
    documents: Documents<SyntaxKey>,
    review: ReviewId,
}

impl Highlighter for SideHighlighter {
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
                    Box::new(SideHighlighter {
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
                if on {
                    self.tell_review_buffers(&running.view, running.root.path());
                }
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

    /// Tells each review's view whose highlighting is on about buffer `id`, which just took a
    /// report: its text may be a file's new side, whose highlights it then gives.
    pub(in crate::app) fn review_buffer_highlighted(&mut self, id: BufferId) {
        for running in &self.reviews.open {
            if self
                .reviews
                .sides
                .get(&running.id)
                .is_some_and(|told| told.on)
                && let Some(side) = self.buffer_side(id, running.root.path())
            {
                running.view.tell(Tell::Buffer(Box::new(side)));
            }
        }
    }

    /// Tells a review's view, whose highlighting just turned on, about every open buffer under
    /// `root`.
    fn tell_review_buffers(&self, view: &View, root: &Path) {
        for (id, _) in self.editor.buffers() {
            if let Some(side) = self.buffer_side(id, root) {
                view.tell(Tell::Buffer(Box::new(side)));
            }
        }
    }

    /// Buffer `id` as a side of a review of the work tree at `root`: its path under `root` as
    /// git spells it, an O(1) copy of its text and highlights, and whether those are whole.
    /// None for a buffer elsewhere, or one that may hold secrets, which shows no highlights.
    fn buffer_side(&self, id: BufferId, root: &Path) -> Option<BufferSide> {
        let buffer = self.editor.buffer(id)?;
        if buffer.verdict().is_withheld() {
            return None;
        }
        let highlights = buffer.highlights()?.clone();
        let path = git_path(buffer.path()?.strip_prefix(root).ok()?)?;
        Some(BufferSide {
            path,
            revision: buffer.revision(),
            text: buffer.text().clone(),
            highlights,
            whole: self.syntax.is_whole(id, buffer.revision()),
        })
    }

    /// Hands a report of a review's side to its view; one of a review closed since is dropped.
    pub(in crate::app) fn review_side_reported(&mut self, review: ReviewId, report: Report<u64>) {
        match self.reviews.index(review) {
            Some(index) => {
                self.reviews.open[index].view.tell(Tell::Syntax(report));
            }
            // Its highlights may be a whole side's: freed off the UI thread.
            None => self.freer.free(report),
        }
    }
}

/// `relative`, a path under a work tree's top, as git spells it: its names joined by `/`.
/// None for one that leaves the top or names nothing.
fn git_path(relative: &Path) -> Option<Box<[u8]>> {
    let mut path = Vec::new();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return None;
        };
        if !path.is_empty() {
            path.push(b'/');
        }
        path.extend_from_slice(name.as_encoded_bytes());
    }
    (!path.is_empty()).then(|| path.into())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::git_path;

    #[test]
    fn a_path_under_the_top_is_spelled_as_git_spells_it() {
        assert_eq!(
            git_path(Path::new("src/a.rs")).as_deref(),
            Some(&b"src/a.rs"[..])
        );
        assert_eq!(git_path(Path::new("a.rs")).as_deref(), Some(&b"a.rs"[..]));
        assert_eq!(git_path(Path::new("../a.rs")), None);
        assert_eq!(git_path(Path::new("")), None);
    }
}
