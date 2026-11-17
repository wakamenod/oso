//! Building a review buffer's text on the view thread (ADR 0122, "The views"): each file's
//! section laid out again only when it changed, the sections laid end to end and compared with
//! the text before, and each side's runs laid over its lines (step 9); or, when only those runs
//! changed, a paint of the last text, which lays out nothing.
//!
//! Must not depend on: `app`, `dispatch`, git or the file system.

use std::sync::Arc;

use oso_core::review::Rows;
use oso_core::syntax::Highlights;

use super::file::section_in;
use super::{State, Text, assemble, project, section};

impl State {
    /// The whole text, laid out from each file's section, those changed laid out again; or,
    /// when nothing in it changed, `None` with the last rows and highlights. The panes' texts
    /// come with it while they are built and changed.
    pub(super) fn build(&mut self) -> Text {
        let reading = self.reading_now();
        let panes = self.panes_text();
        if !self.dirty
            && let Some((rows, highlights)) = self.paint()
        {
            return Text {
                text: None,
                rows: rows.clone(),
                highlights: highlights.clone(),
                panes,
                carried: std::mem::take(&mut self.carried),
                reading,
                folded: self.folded,
                highlighted: self.highlighting.progress(),
            };
        }
        let mut sections: Vec<Arc<section::Rendered>> = Vec::with_capacity(self.order.len() + 2);
        sections.push(Arc::new(section::title(&self.title, &self.said_now())));
        let mut size = sections[0].bytes.len();
        let folded = self.folded;
        let mut paths: Vec<Option<&Box<[u8]>>> = vec![None];
        for path in &self.order {
            let Some(file) = self.files.get_mut(path) else {
                continue;
            };
            if let Some(reading) = &file.reading {
                self.highlighting.reading(path, reading);
            }
            paths.push(Some(path));
            // A file past the limit shows its header alone; one read meanwhile is laid out anew.
            let room = self.max_text.saturating_sub(size);
            let mut kept = file.rendered.unified.take();
            let rendered = section_in(&mut kept, room, || file.render(room, folded));
            file.rendered.unified = kept;
            size += rendered.bytes.len();
            sections.push(rendered);
        }
        for (path, gone) in &mut self.gone {
            let room = self.max_text.saturating_sub(size);
            let (cards, withheld) = (&gone.cards, gone.withheld);
            let rendered = section_in(&mut gone.rendered.unified, room, || {
                section::gone(path, cards, room, folded, withheld)
            });
            size += rendered.bytes.len();
            sections.push(rendered);
        }
        let laid: Vec<&section::Rendered> = sections.iter().map(|rendered| &**rendered).collect();
        let mut built = assemble::assemble(&laid, &self.text);
        let paths: Vec<Option<Box<[u8]>>> = paths.into_iter().map(|path| path.cloned()).collect();
        self.lay_runs(&sections, paths, &mut built.highlights);
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
            carried: std::mem::take(&mut self.carried),
            reading,
            folded,
            highlighted: self.highlighting.progress(),
        }
    }

    /// The last rows and highlights, with the sides' runs that changed since laid over the
    /// sections they touch, the rest shared: a paint that lays out nothing. `None` before the
    /// first text.
    fn paint(&mut self) -> Option<(Rows, Highlights)> {
        let (rows, highlights) = self.last.as_mut()?;
        let changed = self.highlighting.take_changed();
        if !changed.is_empty() {
            for (path, start, rendered) in &self.placed {
                if changed.contains(path) {
                    let sides = self.highlighting.sides(path);
                    project::overlay(highlights, *start, &rendered.slots, sides);
                }
            }
        }
        Some((rows.clone(), highlights.clone()))
    }

    /// Lays each file's sides' runs over its section, `sections` being the text's in order and
    /// `paths` their files' (none for the title's), and keeps where each starts for a paint.
    fn lay_runs(
        &mut self,
        sections: &[Arc<section::Rendered>],
        paths: Vec<Option<Box<[u8]>>>,
        highlights: &mut Highlights,
    ) {
        self.highlighting.take_changed();
        self.placed.clear();
        let mut start = 0;
        let paths = paths.into_iter().chain(std::iter::repeat_with(|| None));
        for (rendered, path) in sections.iter().zip(paths) {
            if let Some(path) = path {
                let sides = self.highlighting.sides(&path);
                if sides.iter().any(Option::is_some) {
                    project::overlay(highlights, start, &rendered.slots, sides);
                }
                if !rendered.slots.is_empty() {
                    self.placed.push((path, start, Arc::clone(rendered)));
                }
            }
            start += rendered.runs.iter().map(|(len, _)| len).sum::<usize>();
        }
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
