//! The backgrounds of a review window's rows in view (ADR 0122, "The views"): added and
//! removed lines, the words refinement marked in them, headers and cards, as cells for a
//! frame.
//!
//! Must not depend on: `command` or `keymap`, or the renderer; this says which cells, and the
//! renderer fills them.
//!
//! - What a row is comes from the review's rows, never from its text.
//! - Only the rows in view are looked at, so a frame costs the same whatever the review's size.

use std::ops::Range;

use crate::editor::Editor;
use crate::frame::{self, CellRect};
use crate::review::{RowKind, Side};
use crate::sum_tree::Bias;
use crate::window::{Screen, WindowId};

/// The backgrounds a review window's rows in view take, for a frame, kept from frame to frame
/// so that a frame allocates nothing once they have grown.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReviewCells {
    pub added: Vec<CellRect>,
    pub removed: Vec<CellRect>,
    pub refine_added: Vec<CellRect>,
    pub refine_removed: Vec<CellRect>,
    pub headers: Vec<CellRect>,
    pub cards: Vec<CellRect>,
    /// Room for the byte spans of a row's refined words.
    spans: Vec<Range<usize>>,
}

impl ReviewCells {
    /// Empties every list, keeping their room.
    pub fn clear(&mut self) {
        self.added.clear();
        self.removed.clear();
        self.refine_added.clear();
        self.refine_removed.clear();
        self.headers.clear();
        self.cards.clear();
        self.spans.clear();
    }
}

/// Which background a row of `kind` takes.
fn band(kind: RowKind) -> Band {
    match kind {
        RowKind::Lines {
            side: Side::Old, ..
        } => Band::Removed,
        RowKind::Lines {
            side: Side::New, ..
        } => Band::Added,
        RowKind::File | RowKind::Hunk { .. } => Band::Header,
        RowKind::Label { .. } | RowKind::Card { .. } => Band::Card,
        RowKind::Note => Band::None,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Band {
    Added,
    Removed,
    Header,
    Card,
    None,
}

impl Editor {
    /// The backgrounds of the review rows the window `id` shows, as `screen` shows it
    /// ([`Editor::screen_of`]), into `out`, emptied first: none for a window that shows no
    /// review buffer. Only the rows in view are looked at.
    pub fn review_cells(&self, window: WindowId, screen: &Screen, out: &mut ReviewCells) {
        out.clear();
        let Some((shown_window, shown, regions)) = self.window_in_tree(window) else {
            return;
        };
        let Some(review) = self.review_of(shown_window.buffer()) else {
            return;
        };
        let Some(rows) = self.review_rows(review) else {
            return;
        };
        let area = regions.text;
        let display = shown.display();
        let text = shown.text();
        let mut spans = std::mem::take(&mut out.spans);
        for (index, screen_row) in screen.rows().iter().enumerate().take(area.rows) {
            let Some(row) = rows.at(screen_row.line) else {
                continue;
            };
            let kind = band(row.kind);
            let list = match kind {
                Band::Added => &mut out.added,
                Band::Removed => &mut out.removed,
                Band::Header => &mut out.headers,
                Band::Card => &mut out.cards,
                Band::None => continue,
            };
            let at = area.row + index;
            match list.last_mut() {
                Some(last) if last.row + last.rows == at => last.rows += 1,
                _ => list.push(CellRect {
                    row: at,
                    column: area.column,
                    rows: 1,
                    columns: area.columns,
                }),
            }
            let refined = rows.refined(screen_row.line);
            if refined.is_empty() || !matches!(kind, Band::Added | Band::Removed) {
                continue;
            }
            let start = text.line_start(screen_row.line);
            let end = text.line_end(screen_row.line);
            spans.clear();
            spans.extend(refined.iter().filter_map(|bytes| {
                let from = (start + bytes.start as usize).min(end);
                let to = (start + bytes.end as usize).min(end);
                let from = text.clip_offset(from, Bias::Left);
                let to = text.clip_offset(to, Bias::Right);
                (from < to).then_some(from..to)
            }));
            let list = if kind == Band::Added {
                &mut out.refine_added
            } else {
                &mut out.refine_removed
            };
            for span in &spans {
                frame::match_cells_on_row(area, (screen, index), &display, span, list);
            }
        }
        spans.clear();
        out.spans = spans;
    }
}
