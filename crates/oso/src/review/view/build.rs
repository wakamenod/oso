//! Building a review buffer's text on the view thread (ADR 0122, "The views"): each file's
//! section laid out again only when it changed, the sections laid end to end and compared with
//! the text before, and each side's runs laid over its lines (step 9); or, when only those runs
//! changed, a paint of the last text, which lays out nothing.
//!
//! Must not depend on: `app`, `dispatch`, git or the file system.

use std::collections::HashSet;
use std::sync::Arc;

use oso_core::review::Rows;
use oso_core::syntax::Highlights;

use super::file::section_in;
use super::project::Run;
use super::{State, Text, assemble, project, section};

impl State {
    /// The whole text, laid out from each file's section, those changed laid out again; or,
    /// when nothing in it changed, `None` with the last rows and highlights. The panes' texts
    /// come with it while they are built and changed.
    pub(super) fn build(&mut self) -> Text {
        // Reports told since the last batch, as when a test tells them and builds.
        self.highlighting.take_reports(&self.order);
        if self.dirty {
            self.refilter();
        }
        // The files whose sides' runs changed, for a paint of the unified text and the panes.
        let changed = self.highlighting.take_changed();
        let reading = self.reading_now();
        let pane_paint = self.panes_paint(&changed);
        let panes = self.panes_text();
        let files = self.files_text();
        if !self.dirty
            && let Some((rows, highlights)) = self.paint(&changed)
        {
            return Text {
                text: None,
                rows: rows.clone(),
                highlights: highlights.clone(),
                panes,
                files,
                pane_paint,
                carried: std::mem::take(&mut self.carried),
                reading,
                folded: self.folded,
                highlighted: self.progress(),
            };
        }
        let mut sections: Vec<Arc<section::Rendered>> = Vec::with_capacity(self.order.len() + 2);
        sections.push(Arc::new(self.title_section()));
        let mut size = sections[0].held();
        let folded = self.folded;
        let mut paths: Vec<Option<&Box<[u8]>>> = vec![None];
        let filtered = self.filtering.on();
        for path in &self.order {
            let Some(file) = self.files.get_mut(path) else {
                continue;
            };
            if filtered && file.judged.hidden() {
                continue;
            }
            paths.push(Some(path));
            // A file past the limit shows its header alone; one read meanwhile is laid out anew.
            let room = self.max_text.saturating_sub(size);
            let mut kept = file.rendered.unified.take();
            let rendered = section_in(&mut kept, room, || file.render(room, folded));
            file.rendered.unified = kept;
            size += rendered.held();
            sections.push(rendered);
        }
        for (path, gone) in &mut self.gone {
            if filtered && gone.judged.hidden() {
                continue;
            }
            let room = self.max_text.saturating_sub(size);
            let (cards, withheld) = (&gone.cards, gone.withheld);
            let rendered = section_in(&mut gone.rendered.unified, room, || {
                section::gone(path, cards, room, folded, withheld)
            });
            size += rendered.held();
            sections.push(rendered);
        }
        let paths: Vec<Option<Box<[u8]>>> = paths.into_iter().map(|path| path.cloned()).collect();
        let runs = self.lay_runs(&sections, paths);
        let laid: Vec<&section::Rendered> = sections.iter().map(|rendered| &**rendered).collect();
        let built = assemble::assemble(&laid, &runs, &self.text);
        // The same text needs no swap, which would only free it again on the UI thread.
        let changed = built.text.is_changed() || !built.valid || !self.valid;
        let text = changed.then_some(built.text);
        self.text = built.rope;
        self.valid = built.valid;
        self.dirty = false;
        self.last = Some((built.rows.clone(), built.highlights.clone()));
        Text {
            text,
            rows: built.rows,
            highlights: built.highlights,
            panes,
            files,
            pane_paint,
            carried: std::mem::take(&mut self.carried),
            reading,
            folded,
            highlighted: self.progress(),
        }
    }

    /// The last rows and highlights, with the runs of the files in `changed` laid over the
    /// sections they touch, the rest shared: a paint that lays out nothing. `None` before the
    /// first text.
    fn paint(&mut self, changed: &HashSet<Box<[u8]>>) -> Option<(Rows, Highlights)> {
        let (rows, highlights) = self.last.as_mut()?;
        if !changed.is_empty() {
            for (path, start, rendered) in &self.placed {
                if changed.contains(path) {
                    let sides = self.highlighting.sides(path);
                    project::paint(highlights, *start, rendered, sides);
                }
            }
        }
        Some((rows.clone(), highlights.clone()))
    }

    /// Each section's runs with its file's sides' merged in, or none for one that keeps its
    /// own, `sections` being the text's in order and `paths` their files' (none for the
    /// title's); and keeps where each section that shows lines starts, for a paint, and which
    /// files those are, as only their sides are highlighted.
    fn lay_runs(
        &mut self,
        sections: &[Arc<section::Rendered>],
        paths: Vec<Option<Box<[u8]>>>,
    ) -> Vec<Option<Vec<Run>>> {
        self.placed.clear();
        let mut laid = HashSet::new();
        let mut runs = Vec::with_capacity(sections.len());
        let mut start = 0;
        let paths = paths.into_iter().chain(std::iter::repeat_with(|| None));
        for (rendered, path) in sections.iter().zip(paths) {
            let mut merged = None;
            if let Some(path) = path
                && !rendered.slots.is_empty()
            {
                merged = project::section_runs(rendered, self.highlighting.sides(&path));
                laid.insert(path.clone());
                self.placed.push((path, start, Arc::clone(rendered)));
            }
            runs.push(merged);
            start += rendered.decoded;
        }
        self.highlighting.laid(laid);
        runs
    }

    /// How far the sides' highlighting is, which only dev builds' timings read.
    fn progress(&self) -> Option<super::Progress> {
        if cfg!(any(test, feature = "dev")) {
            self.highlighting.progress()
        } else {
            None
        }
    }

    /// The review's own section: its title, the message of the commit it reviews, and what is
    /// said under it.
    pub(super) fn title_section(&self) -> section::Rendered {
        section::title(&self.title, self.message.as_deref(), &self.said_now())
    }

    /// What the title says under it: what was said, and what the listing left out.
    pub(super) fn said_now(&self) -> Vec<String> {
        let mut said = self.said.clone();
        if self.more > 0 {
            said.push(format!("{} more files are not listed", self.more));
        }
        if self.cut {
            said.push("more files may not be listed: git's listing was cut short".to_owned());
        }
        said.extend(self.filter_note());
        said
    }

    /// How many of the files listed are read, while a reading runs.
    pub(super) fn reading_now(&self) -> Option<(usize, usize)> {
        self.reading.then(|| {
            let read = self
                .files
                .values()
                .filter(|file| file.reading.is_some())
                .count();
            (read, self.order.len())
        })
    }
}
