//! The cells isearch's matches in view take (ADR 0082), a literal string's or a regexp's,
//! for the renderer and the text-grid snapshots, as the region's are in `view`, over the
//! view rows a frame shows (ADR 0104).
//!
//! Must not depend on: `command` or `keymap`, or the host's window or threads.
//!
//! Only the rows in view are searched, each widened by what a literal string can reach, and a
//! regexp's read a little around them, within [`FRAME_BUDGET`], so a frame costs as much as the
//! view and the string, not the buffer.

use std::ops::Range;

use crate::buffer::BufferId;
use crate::display_map::DisplayPoint;
use crate::editor::Editor;
use crate::frame::{self, CellRect};
use crate::search;
use crate::sum_tree::Bias;
use crate::window::{Screen, WindowId};

/// The most matches in view a frame highlights, so that their rectangles leave room for
/// the rest of the frame.
const MAX_MATCHES_SHOWN: usize = 2048;

/// The most text a frame searches for its highlights.
const FRAME_BUDGET: usize = 256 * 1024;

/// The cells of a frame's matches, and room for working them out, kept from frame to frame
/// so that a frame allocates nothing once they have grown.
#[derive(Debug, Default)]
pub struct MatchCells {
    /// The current match's cells.
    pub current: Vec<CellRect>,
    /// The other matches' cells.
    pub others: Vec<CellRect>,
    pub(in crate::editor) spans: Vec<Range<usize>>,
    pub(in crate::editor) found: Vec<Range<usize>>,
    /// What a project search's preview cells were worked out for, so that a frame that shows
    /// the same works out nothing (ADR 0114).
    pub(in crate::editor) preview_key: Option<crate::editor::project_search::PreviewCellsKey>,
}

/// What [`Editor::match_cells`] looks for: a query, the current match, and a scope.
pub(in crate::editor) type MatchQuery<'a> = (
    &'a search::Needle,
    Option<&'a Range<usize>>,
    Option<&'a [Range<usize>]>,
);

impl Editor {
    /// The cells the matches in view take in the text area of the tiled window `id`, as
    /// `screen` shows it ([`Editor::screen_of`]), into `cells`, emptied first. isearch runs in
    /// the selected window, and only it shows them (ADR 0112).
    pub fn isearch_cells(&self, id: WindowId, screen: &Screen, cells: &mut MatchCells) {
        cells.current.clear();
        cells.others.clear();
        let Some(state) = &self.isearch else {
            return;
        };
        let Some(needle) = state.needle() else {
            return;
        };
        let current = state.current.as_ref().filter(|_| !state.failing);
        self.match_cells(id, state.buffer, screen, (needle, current, None), cells);
    }

    /// The cells of a query's matches in view in the tiled window `id`, if it is the selected
    /// one and shows `buffer`, within a scope if given, the current one apart, into `cells`,
    /// emptied first: isearch's and query-replace's (ADR 0113).
    pub(in crate::editor) fn match_cells(
        &self,
        id: WindowId,
        buffer: BufferId,
        screen: &Screen,
        (query, current, scope): MatchQuery<'_>,
        cells: &mut MatchCells,
    ) {
        let MatchCells {
            current: current_cells,
            others,
            spans,
            found,
            ..
        } = cells;
        current_cells.clear();
        others.clear();
        spans.clear();
        found.clear();
        let Some((window, shown, regions)) = self.selected_in_tree(id) else {
            return;
        };
        if window.buffer() != buffer {
            return;
        }
        let display = shown.display();
        let (text, invalid) = (shown.text(), shown.invalid_bytes());
        let area = regions.text;
        let reach = query.reach();
        // Each view row's columns in view, widened, and joined where they meet.
        for row in screen.rows().iter().take(area.rows) {
            let offset = |column| {
                display.display_point_to_offset(DisplayPoint::new(row.line, column), Bias::Left)
            };
            let drawn = screen.drawn(row);
            let start = offset(drawn.start).saturating_sub(reach);
            let end = offset(drawn.end).saturating_add(reach).min(text.len());
            match spans.last_mut() {
                Some(last) if start <= last.end => last.end = last.end.max(end),
                _ => spans.push(start..end),
            }
        }
        if let Some(scope) = scope {
            // Only what is in both, so a match outside the scope shows nothing.
            let view = std::mem::take(spans);
            for span in &view {
                for range in scope {
                    let (start, end) = (span.start.max(range.start), span.end.min(range.end));
                    if start < end {
                        spans.push(start..end);
                    }
                }
            }
        }
        let limits = (MAX_MATCHES_SHOWN, FRAME_BUDGET);
        query.matches_in((text, invalid), spans, limits, found);
        // The current one alone, and the rest without what overlaps it.
        if let Some(range) = current {
            found.retain(|other| other.end <= range.start || other.start >= range.end);
            let one = std::slice::from_ref(range);
            frame::match_cells(area, screen, &display, one, current_cells);
        }
        frame::match_cells(area, screen, &display, found, others);
    }
}
