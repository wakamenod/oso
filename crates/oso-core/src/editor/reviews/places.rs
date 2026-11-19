//! Where a review's windows are kept across a new text (ADR 0122, "Refresh while the agent
//! edits", step 4): each window's cursors, marks and first row by anchor, a line of a side or
//! a row of a header, a note or a card, never by row number, so that a refresh moves nothing
//! the person has.
//!
//! Must not depend on: `command` or `keymap`, or the host's threads. The host says where each
//! line of a side went, by the carry of its file.
//!
//! - A line of a side follows the host's map: to the line it is now, or the nearest line that
//!   stayed. One the new text does not show, or one with nothing near, goes to the nearest row
//!   its side shows; a row the new text no longer holds goes to its file's header.
//! - A gap's row stays on the gap that still hides any of its lines, as the context keys show
//!   some of them; once none is hidden, it goes to the first line it hid.
//! - A cursor keeps its column. The selections are made again from their places, so that two
//!   the text change put on one offset are not paired with each other's places.
//! - A selection whose place the new text no longer shows, a filler row's or a file's no
//!   longer listed, goes; if none has a place, the window keeps those the text change carried.
//! - A window with more than [`MAX_PLACED`] selections keeps those the text change carried,
//!   so that the cost stays bounded; its first row is still kept.

use super::super::Editor;
use crate::buffer::BufferId;
use crate::display_map::DisplayPoint;
use crate::review::{Place, Rows, Side};
use crate::selection::{Selection, SelectionSet};
use crate::sum_tree::Bias;
use crate::window::Scroll;

/// How many selections of one window are kept by place.
const MAX_PLACED: usize = 256;

/// Where one line was: its file, its place there and a column.
type At = (Box<[u8]>, Place, usize);

/// One window's places, kept before a new text goes in.
pub(super) struct Kept {
    index: usize,
    /// Each selection's cursor and mark, and which was primary; empty past [`MAX_PLACED`].
    selections: Vec<(Option<At>, Option<At>)>,
    primary: usize,
    top: Option<At>,
}

/// How a review says where a line of a side of a file is now: `None` for one that went with
/// nothing near, or that it cannot say.
pub type LineMoves<'a> = &'a dyn Fn(&[u8], Side, u32) -> Option<u32>;

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
            .map(|(index, window)| {
                let set = &window.selections;
                let selections = if set.len() <= MAX_PLACED {
                    let one = |it: &Selection| (at(it.cursor), it.mark.and_then(&at));
                    set.iter().map(one).collect()
                } else {
                    Vec::new()
                };
                Kept {
                    index,
                    selections,
                    primary: set.primary_index(),
                    top: at(window.scroll().start),
                }
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
        moved: LineMoves<'_>,
    ) {
        let Some(shown) = self.buffer(buffer) else {
            return;
        };
        let display = shown.display();
        let line_of = |(path, place, column): &At| -> Option<(usize, usize)> {
            let line = match *place {
                Place::Line { side, number } => {
                    let now = moved(path, side, number);
                    let found =
                        now.and_then(|now| rows.find(path, Place::Line { side, number: now }));
                    found.or_else(|| rows.nearest(path, side, now.unwrap_or(number)))
                }
                // A gap opened whole goes to the first line it hid.
                Place::Gap { old, .. } => {
                    (rows.find(path, *place)).or_else(|| rows.nearest(path, Side::Old, old))
                }
                row => rows.find(path, row),
            };
            Some((line.or_else(|| rows.file_start(path))?, *column))
        };
        let offset = |(line, column): (usize, usize)| {
            display.display_point_to_offset(DisplayPoint::new(line, column), Bias::Left)
        };
        for kept in kept {
            let Some(window) = self.windows.get_mut(kept.index) else {
                continue;
            };
            if window.buffer() != buffer {
                continue;
            }
            let mut primary = 0;
            let mut placed = Vec::with_capacity(kept.selections.len());
            for (at, (cursor, mark)) in kept.selections.iter().enumerate() {
                let Some(cursor) = cursor.as_ref().and_then(line_of) else {
                    continue;
                };
                if at == kept.primary {
                    primary = placed.len();
                }
                let mut selection = Selection::at(offset(cursor));
                selection.mark = mark.as_ref().and_then(line_of).map(offset);
                placed.push(selection);
            }
            if let Some(mut set) = SelectionSet::from_parts(placed, primary) {
                set = set.with_ring(*window.selections.mark_ring());
                set.set_mark_active(window.selections.mark_active());
                window.selections = set;
            } else if !kept.selections.is_empty() {
                // None has a place, as their file is hidden now (step 6b): the cursor goes to
                // the header of the file the text change carried it into.
                let line = (display.offset_to_display_point(window.selections.primary().cursor))
                    .row
                    .min(rows.lines().saturating_sub(1));
                let header = (rows.at(line)).and_then(|row| rows.file_start(row.path));
                if let Some(header) = header {
                    window.selections = SelectionSet::new(offset((header, 0)));
                }
            }
            if let Some((line, _)) = kept.top.as_ref().and_then(line_of) {
                let start = offset((line, 0));
                let left = window.scroll().left;
                window.set_scroll(Scroll { start, left });
            }
        }
    }
}
