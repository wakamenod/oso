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
use oso_core::window::{Scroll, WindowId};

/// Positions waiting for their file to load, since a cursor or a mark cannot sit in
/// text that has not arrived. The file may also have changed since the export, so
/// they are moved to the nearest place that exists. Carried text is all there, so
/// its positions apply at once.
#[derive(Debug)]
pub struct Restore {
    /// The window the positions are for (ADR 0112).
    pub window: WindowId,
    pub buffer: BufferId,
    pub selections: SelectionSet,
    pub scroll: Scroll,
    /// Whether the scroll's start is a logical line, from a state before version 3, which
    /// kept the top as a row.
    pub top_is_row: bool,
    /// Where the window was when the positions began to wait, if they are to be dropped once
    /// a person moves it: a later load must not take it from where it was taken (ADR 0112).
    pub untouched: Option<(SelectionSet, Scroll)>,
}

/// `offset` moved to the start of the display cluster it is in, or the end.
fn clip(display: &DisplaySnapshot, offset: usize) -> usize {
    let point: DisplayPoint = display.offset_to_display_point(offset);
    display.display_point_to_offset(point, Bias::Left)
}

impl Restore {
    /// Whether the window still is where it was when the positions began to wait, or shows
    /// another buffer, whose place it keeps for this one: whether the positions still stand.
    pub fn stands(&self, editor: &Editor) -> bool {
        let Some(window) = editor.window_by_id(self.window) else {
            return false;
        };
        window.buffer() != self.buffer
            || self.untouched.as_ref().is_none_or(|(selections, scroll)| {
                window.selections == *selections && window.scroll() == *scroll
            })
    }

    /// Moves the window to the positions if the buffer holds them by now, or, if the
    /// window has gone to another buffer meanwhile, the place it keeps for this one, which
    /// showing it again gives back (ADR 0090). The window need not be selected (ADR 0112).
    /// Returns whether this is over: restored, or the window is gone, keeps no place for the
    /// buffer, or was moved meanwhile, when they no longer stand.
    pub fn try_apply(&self, editor: &mut Editor) -> bool {
        if !self.stands(editor) {
            return true;
        }
        let Some(window) = editor.window_by_id(self.window) else {
            return true;
        };
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
            editor.restore_positions_in(self.window, selections, scroll);
        } else {
            editor.restore_kept_place_in(self.window, self.buffer, selections, scroll);
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
