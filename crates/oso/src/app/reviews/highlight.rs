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
use std::path::Path;

use oso_core::buffer::{BufferId, Verdict};
use oso_core::editor::{Editor, ReviewId};
use oso_core::rope::Rope;
use oso_syntax::{Language, Refused, Report};

use super::super::App;
use super::super::syntax::{Documents, Syntax, SyntaxKey};
use crate::git::name_of;
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
                && let Some(side) = buffer_side(&self.editor, &self.syntax, id, running.root.path())
            {
                running.view.tell(Tell::Buffer(Box::new(side)));
            }
        }
    }

    /// Tells a review's view, whose highlighting just turned on, about every open buffer under
    /// `root`.
    fn tell_review_buffers(&self, view: &View, root: &Path) {
        for (id, _) in self.editor.buffers() {
            if let Some(side) = buffer_side(&self.editor, &self.syntax, id, root) {
                view.tell(Tell::Buffer(Box::new(side)));
            }
        }
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

/// Buffer `id` as a side of a review of the work tree at `root`: its path under `root` as git
/// spells it, an O(1) copy of its text and highlights, and whether those are whole. None for a
/// buffer elsewhere, or one that may hold secrets, which shows no highlights either.
fn buffer_side(editor: &Editor, syntax: &Syntax, id: BufferId, root: &Path) -> Option<BufferSide> {
    let buffer = editor.buffer(id)?;
    if buffer.verdict().is_withheld() {
        return None;
    }
    let highlights = buffer.highlights()?.clone();
    let path = name_of(buffer.path()?.strip_prefix(root).ok()?)?.into_boxed_slice();
    Some(BufferSide {
        path,
        revision: buffer.revision(),
        text: buffer.text().clone(),
        highlights,
        whole: syntax.is_whole(id, buffer.revision()),
        language: Language::of(buffer.mode()),
    })
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use oso_core::buffer::{Buffer, BufferId};
    use oso_core::display_map::TabWidth;
    use oso_core::editor::Editor;
    use oso_core::load::decode_all;
    use oso_syntax::{Outcome, Report};

    use super::super::super::syntax::Syntax;
    use super::buffer_side;

    /// An editor with `path` visited, judged as its name says, and highlighted.
    fn highlighted(path: &str) -> (Editor, Syntax, BufferId) {
        let path = PathBuf::from(path);
        let mut buffer = Buffer::visiting(path.clone());
        buffer.judge_secret(Some(&[path]));
        buffer.apply_load(decode_all(b"fn a() {}\n", TabWidth::default()));
        let mut editor = Editor::with_buffer(Buffer::from_text("*scratch*", ""));
        let id = editor.add_buffer(buffer);
        editor.show(id);
        let mut syntax = Syntax::default();
        if let Some(job) = syntax.orders(&editor).jobs.pop() {
            let report = Report {
                buffer: id,
                epoch: job.epoch,
                stamp: 1,
                revision: job.revision,
                outcome: Outcome::Highlights(oso_core::syntax::Highlights::new(job.text.len())),
                done: true,
            };
            syntax.take(&mut editor, report);
        }
        (editor, syntax, id)
    }

    #[test]
    fn an_ordinary_buffer_under_the_top_is_offered_whole_and_no_other() {
        let top = Path::new("/p");
        let (editor, syntax, id) = highlighted("/p/src/a.rs");
        let side = buffer_side(&editor, &syntax, id, top).expect("offered");
        assert_eq!(&*side.path, b"src/a.rs");
        assert!(side.whole);
        assert_eq!(side.language, Some(oso_syntax::Language::Rust));
        assert!(
            buffer_side(&editor, &syntax, id, Path::new("/q")).is_none(),
            "elsewhere"
        );
        // A name that may hold secrets: never offered, as it is never highlighted either.
        let (editor, syntax, id) = highlighted("/p/.ssh/config.rs");
        assert!(buffer_side(&editor, &syntax, id, top).is_none());
        // One not judged yet.
        let mut editor = Editor::with_buffer(Buffer::from_text("*scratch*", ""));
        let mut unjudged = Buffer::visiting(PathBuf::from("/p/b.rs"));
        unjudged.apply_load(decode_all(b"fn b() {}\n", TabWidth::default()));
        let id = editor.add_buffer(unjudged);
        assert!(buffer_side(&editor, &Syntax::default(), id, top).is_none());
    }
}
