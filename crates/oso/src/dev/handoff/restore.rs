//! Positions a new instance puts back once their buffer holds them (ADR 0047, ADR 0090,
//! ADR 0103): the cursors, the marks and the window's top, each moved to where the text is
//! now.
//!
//! Must not depend on: the control socket, the window system or threads; it reads the editor
//! it is handed.

use oso_core::buffer::BufferId;
use oso_core::display_map::{DisplayPoint, DisplaySnapshot};
use oso_core::editor::Editor;
use oso_core::selection::{MarkRing, Selection, SelectionSet};
use oso_core::sum_tree::Bias;
use oso_core::window::Scroll;

/// Positions waiting for their file to load, since a cursor or a mark cannot sit in
/// text that has not arrived. The file may also have changed since the export, so
/// they are moved to the nearest place that exists. Carried text is all there, so
/// its positions apply at once.
#[derive(Debug)]
pub struct Restore {
    pub buffer: BufferId,
    pub selections: SelectionSet,
    pub scroll: Scroll,
    /// Whether the scroll's start is a logical line, from a state before version 3, which
    /// kept the top as a row.
    pub top_is_row: bool,
}

/// `offset` moved to the start of the display cluster it is in, or the end.
fn clip(display: &DisplaySnapshot, offset: usize) -> usize {
    let point: DisplayPoint = display.offset_to_display_point(offset);
    display.display_point_to_offset(point, Bias::Left)
}

impl Restore {
    /// Moves the window to the positions if the buffer holds them by now, or, if the
    /// window has gone to another buffer meanwhile, the place it keeps for this one, which
    /// showing it again gives back (ADR 0090). Returns whether this is over: restored, or the
    /// window keeps no place for the buffer.
    pub fn try_apply(&self, editor: &mut Editor) -> bool {
        let window = editor.tiled().0;
        let shown = window.buffer() == self.buffer;
        let kept = window.places().any(|(buffer, _)| buffer == self.buffer);
        let Some(buffer) = editor.buffer(self.buffer).filter(|_| shown || kept) else {
            return true;
        };
        let selections = self.selections.iter();
        let marks = selections.flat_map(|s| [Some(s.cursor), s.mark]).flatten();
        // The top too, once it is an offset.
        let top = (!self.top_is_row).then_some(self.scroll.start);
        let needed = marks
            .chain(self.selections.mark_ring().iter())
            .chain(top)
            .max()
            .unwrap_or(0);
        if buffer.loading().is_some() && buffer.text().len() < needed {
            return false;
        }
        let display = buffer.display();
        let selections = self.clipped(&display);
        let mut scroll = self.scroll;
        scroll.start = if self.top_is_row {
            let row = scroll.start.min(display.row_count().saturating_sub(1));
            oso_core::motion::line_start(&display, row)
        } else {
            clip(&display, scroll.start)
        };
        if shown {
            editor.restore_positions(selections, scroll);
        } else {
            editor.restore_kept_place(self.buffer, selections, scroll);
        }
        true
    }

    /// The positions, each moved to the start of the display cluster it is in, or the end.
    fn clipped(&self, display: &DisplaySnapshot) -> SelectionSet {
        let clip = |offset: usize| clip(display, offset);
        let selections = self
            .selections
            .iter()
            .map(|selection| Selection {
                cursor: clip(selection.cursor),
                mark: selection.mark.map(clip),
                goal: selection.goal,
            })
            .collect();
        let mut set = SelectionSet::from_parts(selections, self.selections.primary_index())
            .unwrap_or_default();
        set.set_mark_active(self.selections.mark_active());
        let ring = MarkRing::from_newest(self.selections.mark_ring().iter().map(clip));
        set.with_ring(ring)
    }
}
