//! The cells isearch's matches in view take (ADR 0082), for the renderer and the text-grid
//! snapshots, as the region's are in `view`.
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

/// The most matches in view a frame highlights, so that their rectangles leave room for
/// the rest of the frame.
const MAX_MATCHES_SHOWN: usize = 2048;

/// The most text a frame searches for its highlights.
const FRAME_BUDGET: usize = 256 * 1024;

impl Editor {
    /// The cells the matches in view take in the tiled window's text area: the current one
    /// into `current`, and the others into `others`, both emptied first.
    pub fn isearch_cells(&self, current: &mut Vec<CellRect>, others: &mut Vec<CellRect>) {
        current.clear();
        others.clear();
        let Some(state) = &self.isearch else {
            return;
        };
        let Some(query) = &state.query else {
            return;
        };
        let (window, buffer) = self.tiled();
        if window.buffer() != state.buffer {
            return;
        }
        let display = buffer.display();
        let (text, invalid) = (buffer.text(), buffer.invalid_bytes());
        let (area, scroll) = (self.frame.window.text, window.scroll());
        let rows = scroll.top
            ..scroll
                .top
                .saturating_add(area.rows)
                .min(display.row_count());
        let reach = query.max_bytes();
        // Each row's columns in view, widened, and joined where they meet.
        let mut spans: Vec<Range<usize>> = Vec::with_capacity(area.rows);
        for row in rows {
            let offset = |column| {
                display.display_point_to_offset(DisplayPoint::new(row, column), Bias::Left)
            };
            let start = offset(scroll.left).saturating_sub(reach);
            let end = offset(scroll.left.saturating_add(area.columns))
                .saturating_add(reach)
                .min(text.len());
            match spans.last_mut() {
                Some(last) if start <= last.end => last.end = last.end.max(end),
                _ => spans.push(start..end),
            }
        }
        let mut found = Vec::new();
        let limits = (MAX_MATCHES_SHOWN, FRAME_BUDGET);
        search::matches_in((text, invalid), query, &spans, limits, &mut found);
        // The current one alone, and the rest without what overlaps it.
        if let Some(range) = state.current.as_ref().filter(|_| !state.failing) {
            found.retain(|other| other.end <= range.start || other.start >= range.end);
            let one = std::slice::from_ref(range);
            frame::match_cells(area, scroll, &display, one, current);
        }
        frame::match_cells(area, scroll, &display, &found, others);
    }
}
