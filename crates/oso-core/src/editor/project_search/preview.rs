//! The preview of a project's search (ADR 0114): the selected row's file around its line, as
//! the shared preview of a list's files shows it ([`crate::editor::file_preview`]), with the
//! row's match as the current one and the query's other matches on the lines shown.
//!
//! Must not depend on: `command` or `keymap`, or the host's threads or I/O; the host reads the
//! file and hands over an excerpt of it.

use std::ops::Range;
use std::path::Path;

use crate::display_map::{DisplayPoint, DisplaySnapshot};
use crate::editor::file_preview::{Lister, PreviewAt, Wanted};
use crate::editor::{Editor, MatchCells, Preview};
use crate::frame::{self, CellRect};
use crate::project::LineQuery;
use crate::rope::Rope;
use crate::sum_tree::Bias;
use crate::window::Screen;

/// How much of a line before and after what its rows draw is matched for the preview's
/// highlights, so that a match that starts or ends out of view shows; one longer than this that
/// starts before what is drawn does not.
const AROUND_DRAWN: usize = 1024;

impl Editor {
    /// The row of the innermost prompt selected, if it searches a project: its search's
    /// generation and its item.
    pub(in crate::editor) fn selected_search_row(&self) -> Option<(u64, usize)> {
        if !self.searches_project() {
            return None;
        }
        let item = self.prompts.last()?.candidates()?.selected_item()?;
        Some((self.project_search.generation, item))
    }

    /// The selected row of a project's search, as its preview wants it.
    pub(in crate::editor) fn search_wanted(&self) -> Option<Wanted<'_>> {
        let key = self.selected_search_row()?;
        let candidates = self.prompts.last()?.candidates()?;
        let text = candidates.get(candidates.selected()?)?.text();
        let place = self.project_search.places.get(key.1)?;
        // A conversation's row has no file (ADR 0145).
        let super::To::File {
            path,
            offset,
            secret,
        } = place.to
        else {
            return None;
        };
        Some(Wanted {
            key,
            root: &self.project_search.root,
            path: Path::new(text.get(..path)?),
            at: PreviewAt::Offset {
                offset,
                line: place.line,
                column: place.column,
            },
            secret,
        })
    }

    /// What the preview of the innermost prompt's selected row shows, if it searches a project.
    pub(in crate::editor) fn search_preview(&self) -> Option<Preview> {
        match self.project_search.source {
            super::Source::Files => self.file_row_preview(Lister::Search),
            super::Source::Conversations(_) => self.conversation_preview(),
            // A buffer's line search shows the buffer from memory (ADR 0154).
            super::Source::Lines(_) => self.line_preview(),
        }
    }

    /// The cells the matches in view take in a search's preview, as `screen` shows it
    /// ([`Editor::preview_screen`]), into `cells`: the selected row's match as the current one,
    /// the query's other matches on the lines shown as the rest. They are worked out again only
    /// once what is shown, its rows or the search change.
    pub fn preview_cells(&self, screen: &Screen, cells: &mut MatchCells) {
        let Some(query) = self.project_search.query.as_deref() else {
            cells.preview_key = None;
            cells.current.clear();
            cells.others.clear();
            return;
        };
        let region = self.prompt_layout().map(|layout| layout.regions.preview);
        let preview = self.search_preview();
        let shown = match preview {
            Some(Preview::Buffer { id, cursor, .. }) => self
                .buffer(id)
                .map(|buffer| (buffer.text(), buffer.display(), cursor, buffer.revision())),
            Some(Preview::Excerpt { cursor, revision }) => self
                .shown_excerpt()
                .map(|(text, display, _)| (text, display, cursor, revision)),
            _ => None,
        };
        let (Some(region), Some(preview), Some((text, display, cursor, revision))) =
            (region, preview, shown)
        else {
            cells.preview_key = None;
            cells.current.clear();
            cells.others.clear();
            return;
        };
        let top = screen
            .rows()
            .first()
            .map(|row| (row.line, screen.drawn(row).start));
        let key = PreviewCellsKey {
            search: self.project_search.generation,
            preview,
            revision,
            region,
            top,
            rows: screen.rows().len(),
        };
        if cells.preview_key == Some(key) {
            return;
        }
        cells.preview_key = Some(key);
        cells.current.clear();
        cells.others.clear();
        let MatchCells {
            current,
            others,
            spans,
            found,
            ..
        } = cells;
        matches_drawn((text, &display), (screen, region.rows), query, spans, found);
        if let Some(index) = found
            .iter()
            .position(|range| range.start <= cursor && cursor < range.end)
        {
            let range = found.remove(index);
            frame::match_cells(region, screen, &display, &[range], current);
        }
        frame::match_cells(region, screen, &display, found, others);
    }
}

/// The query's matches in what `screen`'s first `rows` rows draw of `text`, shown as `display`
/// lays it out, into `found`, cleared first, in order: each line matched once, over what its
/// rows draw and [`AROUND_DRAWN`] either side, with `spans` as room.
fn matches_drawn(
    (text, display): (&Rope, &DisplaySnapshot),
    (screen, rows): (&Screen, usize),
    query: &LineQuery,
    spans: &mut Vec<Range<usize>>,
    found: &mut Vec<Range<usize>>,
) {
    found.clear();
    // What each line shown draws, its rows joined.
    let mut drawn: Vec<(usize, Range<usize>)> = Vec::new();
    for row in screen.rows().iter().take(rows) {
        let offset = |column| {
            display.display_point_to_offset(DisplayPoint::new(row.line, column), Bias::Left)
        };
        let columns = screen.drawn(row);
        let range = offset(columns.start)..offset(columns.end);
        match drawn.last_mut() {
            Some((line, last)) if *line == row.line => {
                last.start = last.start.min(range.start);
                last.end = last.end.max(range.end);
            }
            _ => drawn.push((row.line, range)),
        }
    }
    let mut line = String::new();
    for (row, range) in drawn {
        // At a character's start, as the chunks read start there.
        let start = text.clip_offset(
            range
                .start
                .saturating_sub(AROUND_DRAWN)
                .max(text.line_start(row)),
            Bias::Left,
        );
        let end = range
            .end
            .saturating_add(AROUND_DRAWN)
            .min(text.line_end(row));
        line.clear();
        for chunk in text.chunks_in_range(start..end) {
            line.push_str(chunk);
        }
        query.highlights(&line, spans);
        // Only what is drawn: the rest of the line is out of view.
        found.extend(
            spans
                .iter()
                .map(|found| start + found.start..start + found.end)
                .filter(|found| found.end > range.start && found.start < range.end),
        );
    }
}

/// What a search's preview cells were worked out for: the search, what the preview shows and
/// at which revision, where, and the rows it shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::editor) struct PreviewCellsKey {
    search: u64,
    preview: Preview,
    revision: u64,
    region: CellRect,
    top: Option<(usize, usize)>,
    rows: usize,
}

mod conversation;

#[cfg(test)]
mod tests;
