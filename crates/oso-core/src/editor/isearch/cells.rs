//! The cells isearch's matches in view take (ADR 0082), for the renderer and the text-grid
//! snapshots, as the region's are in `view`, over the view rows a frame shows (ADR 0104).
//!
//! Must not depend on: `command` or `keymap`, or the host's window or threads.
//!
//! Only the rows in view are searched, each widened by what the string can reach, within
//! [`FRAME_BUDGET`], so a frame costs as much as the view and the string, not the buffer.

use std::ops::Range;

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
    spans: Vec<Range<usize>>,
    found: Vec<Range<usize>>,
}

impl Editor {
    /// The cells the matches in view take in the text area of the tiled window `id`, as
    /// `screen` shows it ([`Editor::screen_of`]), into `cells`, emptied first. isearch runs in
    /// the selected window, and only it shows them (ADR 0112).
    pub fn isearch_cells(&self, id: WindowId, screen: &Screen, cells: &mut MatchCells) {
        let MatchCells {
            current,
            others,
            spans,
            found,
        } = cells;
        current.clear();
        others.clear();
        spans.clear();
        found.clear();
        let Some(state) = &self.isearch else {
            return;
        };
        let Some(query) = &state.query else {
            return;
        };
        let Some((window, buffer, regions)) = self.selected_tiled(id) else {
            return;
        };
        if window.buffer() != state.buffer {
            return;
        }
        let display = buffer.display();
        let (text, invalid) = (buffer.text(), buffer.invalid_bytes());
        let area = regions.text;
        let reach = query.max_bytes();
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
        let limits = (MAX_MATCHES_SHOWN, FRAME_BUDGET);
        search::matches_in((text, invalid), query, spans, limits, found);
        // The current one alone, and the rest without what overlaps it.
        if let Some(range) = state.current.as_ref().filter(|_| !state.failing) {
            found.retain(|other| other.end <= range.start || other.start >= range.end);
            let one = std::slice::from_ref(range);
            frame::match_cells(area, screen, &display, one, current);
        }
        frame::match_cells(area, screen, &display, found, others);
    }
}
