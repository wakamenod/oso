//! The cells a jump draws in a tiled window (ADR 0116): its string's matches while it reads,
//! in isearch's other-match fill, and its labels after, over their targets' first cells.
//!
//! Must not depend on: `command` or `keymap`, or the host's window or threads.
//!
//! A frame looks at the candidates a key found, at most `MAX_LABELLED`, with the display
//! points the key worked out, and finds and converts none again, so it costs as much as they
//! do, whatever the buffer's size.

use std::ops::Range;

use crate::display_map::DisplayPoint;
use crate::editor::Editor;
use crate::frame::{self, CellRect};
use crate::jump::Stage;
use crate::window::{Screen, WindowId};

/// What a jump draws in a window, kept from frame to frame so that a frame allocates nothing
/// once they have grown.
#[derive(Debug, Default)]
pub struct JumpCells {
    /// The cells of the string's matches while it is read.
    pub matches: Vec<CellRect>,
    /// The labels in view, by row and then column, none over another.
    pub labels: Vec<LabelCells>,
    /// The labels' keys, which each label's `keys` is a range of.
    pub text: String,
    ranges: Vec<(Range<usize>, DisplayPoint, DisplayPoint)>,
}

/// A label in view: its cells, one row and one cell a key, from its target's first cell and
/// cut where its row's text stops, before a wrapped row's mark, or at the next label, and the
/// keys left to type of it, the first the one to type next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LabelCells {
    pub cells: CellRect,
    /// Its keys in [`JumpCells::text`].
    pub keys: Range<usize>,
}

impl Editor {
    /// What a jump draws in the text area of the tiled window `id`, as `screen` shows it
    /// ([`Editor::screen_of`]), into `cells`, emptied first.
    pub fn jump_cells(&self, id: WindowId, screen: &Screen, cells: &mut JumpCells) {
        let JumpCells {
            matches,
            labels,
            text,
            ranges,
        } = cells;
        matches.clear();
        labels.clear();
        text.clear();
        ranges.clear();
        // A jump a change ended draws nothing while it waits for the key it takes.
        let Some(jump) = self.jump.as_ref().filter(|jump| jump.ended.is_none()) else {
            return;
        };
        let Some((_, buffer, regions)) = self.window_in_tree(id) else {
            return;
        };
        let display = buffer.display();
        let area = regions.text;
        let here = |window: WindowId| window == id;
        match jump.stage {
            Stage::Reading => {
                ranges.extend(
                    (jump.candidates.iter())
                        .filter(|candidate| here(candidate.window))
                        .map(|c| (c.range.clone(), c.start, c.end)),
                );
                ranges.sort_unstable_by_key(|(range, ..)| range.start);
                frame::match_cells_at(area, screen, &display, ranges, matches);
            }
            Stage::Labels => {
                for (candidate, rest) in jump.shown_labels() {
                    if !here(candidate.window) {
                        continue;
                    }
                    let point = candidate.start;
                    let Some(mut rect) = frame::cursor_rect(area, screen, point, rest.len()) else {
                        continue;
                    };
                    // Cut where its row's text stops, so that it leaves a wrapped row's mark.
                    let row = &screen.rows()[rect.row - area.row];
                    let room = screen.drawn(row).end.saturating_sub(point.column);
                    rect.columns = rect.columns.min(room.max(1));
                    let start = text.len();
                    let shown = &rest[..rect.columns.min(rest.len())];
                    text.extend(shown.iter().map(|&key| char::from(jump.keys.byte(key))));
                    labels.push(LabelCells {
                        cells: rect,
                        keys: start..text.len(),
                    });
                }
                labels.sort_unstable_by_key(|label| (label.cells.row, label.cells.column));
                cut_where_labels_meet(labels);
            }
        }
    }
}

/// Cuts each label, in order by row and column, at the next one on its row, and drops one
/// left with no cell.
fn cut_where_labels_meet(labels: &mut Vec<LabelCells>) {
    for at in 1..labels.len() {
        let next = labels[at].cells;
        let label = &mut labels[at - 1];
        if label.cells.row == next.row {
            let room = next.column.saturating_sub(label.cells.column);
            if label.cells.columns > room {
                label.cells.columns = room;
                label.keys.end = label.keys.start + room;
            }
        }
    }
    labels.retain(|label| label.cells.columns > 0);
}
