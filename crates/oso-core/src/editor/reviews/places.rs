//! Where a review's windows are kept across a new text (ADR 0122, "Refresh while the agent
//! edits", step 4): each window's cursors and first row by anchor, a line of a side or a row of
//! a header, a note or a card, never by row number, so that a refresh moves nothing the person
//! has.
//!
//! Must not depend on: `command` or `keymap`, or the host's threads. The host says where each
//! line of a side went, by the carry of its file.
//!
//! - A line of a side follows the host's map: to the line it is now, or the nearest line that
//!   stayed. A row is found again by its place; one the new text no longer holds goes to its
//!   file's header.
//! - A cursor keeps its column; its mark stays where the text change carried it.
//! - A place in a file the new text no longer shows stays where the text change carried it.

use super::super::Editor;
use crate::buffer::BufferId;
use crate::display_map::DisplayPoint;
use crate::review::{Place, Rows, Side};
use crate::sum_tree::Bias;
use crate::window::Scroll;

/// Where one line was: its file, its place there and a column.
type At = (Box<[u8]>, Place, usize);

/// One window's places, kept before a new text goes in.
pub(super) struct Kept {
    index: usize,
    cursors: Vec<Option<At>>,
    top: Option<At>,
}

/// How a review says where a line of a side of a file is now: `None` for one that went with
/// nothing near.
pub type Moved<'a> = &'a dyn Fn(&[u8], Side, u32) -> Option<u32>;

impl Editor {
    /// The places of every window showing `buffer`, by its rows `rows`.
    pub(super) fn keep_places(&self, buffer: BufferId, rows: &Rows) -> Vec<Kept> {
        let Some(shown) = self.buffer(buffer) else {
            return Vec::new();
        };
        let display = shown.display();
        let at = |offset: usize| -> Option<At> {
            let point = display.offset_to_display_point(offset);
            let (path, place) = rows.place(point.row)?;
            Some((path.into(), place, point.column))
        };
        (self.windows.iter().enumerate())
            .filter(|(_, window)| window.buffer() == buffer)
            .map(|(index, window)| Kept {
                index,
                cursors: window.selections.iter().map(|it| at(it.cursor)).collect(),
                top: at(window.scroll().start),
            })
            .collect()
    }

    /// Puts each window of `kept` back at its places in `buffer`'s new text, whose rows are
    /// `rows`, a line of a side moved by `moved`.
    pub(super) fn put_places(
        &mut self,
        buffer: BufferId,
        kept: Vec<Kept>,
        rows: &Rows,
        moved: Moved<'_>,
    ) {
        let Some(shown) = self.buffer(buffer) else {
            return;
        };
        let display = shown.display();
        let line_of = |(path, place, column): &At| -> Option<(usize, usize)> {
            let place = match *place {
                Place::Line { side, number } => Place::Line {
                    side,
                    number: moved(path, side, number)?,
                },
                row => row,
            };
            let line = rows.find(path, place).or_else(|| rows.file_start(path))?;
            Some((line, *column))
        };
        let offset = |(line, column): (usize, usize)| {
            display.display_point_to_offset(DisplayPoint::new(line, column), Bias::Left)
        };
        for Kept {
            index,
            cursors,
            top,
        } in kept
        {
            let Some(window) = self.windows.get_mut(index) else {
                continue;
            };
            if window.buffer() != buffer {
                continue;
            }
            let mut at = cursors.iter();
            window.selections.update(|selection| {
                if let Some(now) = at.next().and_then(|was| was.as_ref()).and_then(line_of) {
                    selection.cursor = offset(now);
                }
                Ok::<(), ()>(())
            });
            if let Some((line, _)) = top.as_ref().and_then(line_of) {
                let start = offset((line, 0));
                let left = window.scroll().left;
                window.set_scroll(Scroll { start, left });
            }
        }
    }
}
