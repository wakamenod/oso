//! The rows near a window's view before a job reports (ADR 0100, ADR 0103): every row of a
//! new width starts provisional, one a logical line, and those the view and its scrolling
//! reach are wrapped on the UI thread, from an anchor, within the command's budget.
//!
//! Must not depend on: windows, threads or the clock; the window says which rows it needs.
//!
//! - An anchor is a row's start, or within a long provisional row, a point at most
//!   [`ANCHOR_BACK`] before the rows wanted: past a run of spaces there, or else a cluster
//!   start. Rows wrap forward from it greedily, so they equal the real ones only from a real
//!   break on. They stay provisional for the job, but are not wrapped again here.
//! - The rows from the anchor join the old ones at the next old row boundary, with a
//!   provisional row between, which is left stale.

use std::ops::Range;

use super::edit::{Fresh, provisional};
use super::job::{SPLICE_COST, WrapMap};
use super::{Builder, Bytes, Row, RowStream, WrapSnapshot};
use crate::display_map::DisplaySnapshot;

/// How far before the rows wanted an anchor in a long provisional row may go, so that the rows
/// just above them, which `M-v` and `C-l` read, come from it too.
#[cfg(not(test))]
const ANCHOR_BACK: usize = 64 * 1024;
/// Small in tests, whose texts are small too.
#[cfg(test)]
const ANCHOR_BACK: usize = 8;

/// How far past that point an anchor looks for a run of spaces to start after.
#[cfg(not(test))]
const SPACE_SEARCH: usize = 4 * 1024;
/// Small in tests, as [`ANCHOR_BACK`] is.
#[cfg(test)]
const SPACE_SEARCH: usize = 4;

/// How many provisional spans one call wraps, each a call's cost.
const PASSES: usize = 4;

impl WrapSnapshot {
    /// The rows of `display` `width` cells wide before any is wrapped: one a logical line,
    /// found from the display map in O(log n).
    pub(super) fn provisional(display: &DisplaySnapshot, width: usize) -> Self {
        let mut fresh = Vec::new();
        provisional(display, (0, display.len()), &mut fresh);
        let mut builder = Builder::new(true);
        for piece in fresh {
            match piece {
                Fresh::Row(row) => builder.row(row),
                Fresh::Lines(lines, bytes) => builder.lines(lines, bytes),
            }
        }
        Self {
            tree: builder.finish().into_iter().collect(),
            width,
        }
    }

    /// The rows with those from `anchor`, a cluster start, wrapped at the width: until they
    /// pass `rows` rows past the one that holds `target`, reach `end`, or spend `budget`. The
    /// row that held `anchor` keeps its part before it as one row. Returns the rows, where the
    /// wrapped ones end, and the old row boundary they join, past a provisional row.
    fn realized(
        &self,
        display: &DisplaySnapshot,
        anchor: usize,
        (target, rows, end): (usize, usize, usize),
        budget: &mut usize,
    ) -> (Self, usize, usize) {
        let from = self.row_span(display, self.row_of(display, anchor)).0;
        let mut fresh = Vec::new();
        if anchor > from {
            fresh.push(Fresh::Row(Row {
                len: u32::try_from(anchor - from).unwrap_or(u32::MAX),
                continues: true,
                tabs: true,
            }));
        }
        *budget = budget.saturating_sub(SPLICE_COST);
        let mut stream = RowStream::new(display, anchor, self.width);
        let (mut reached, mut past) = (anchor, 0);
        let mut ended = true;
        while let Some(row) = stream.next() {
            let start = reached;
            fresh.push(Fresh::Row(row));
            *budget = budget.saturating_sub(row.len as usize);
            reached = stream.position().0;
            if start > target {
                past += 1;
            }
            if reached >= end || (reached > target && past >= rows) || *budget == 0 {
                ended = false;
                break;
            }
        }
        let old_end = self.tree.summary().bytes;
        let join = if ended || reached >= display.len() {
            // The empty line after a last line break, if the rows stopped before it.
            fresh.extend(stream.map(Fresh::Row));
            reached = display.len();
            old_end
        } else {
            self.join_old((display, display), (reached, reached), 0, &mut fresh)
                .0
        };
        let mut tree = self.tree.clone();
        let (first, last, items) = self.splice(display, (from, join), &fresh);
        tree.replace_items(Bytes(first)..Bytes(last), items);
        let wrap = Self {
            tree,
            width: self.width,
        };
        (wrap, reached, join)
    }

    /// Where rows wanted from `target` start wrapping: the start of its row, if that is at
    /// most [`ANCHOR_BACK`] before it or before `floor`, where the provisional rows start;
    /// otherwise past a run of spaces near that bound, after it or else before it, so that no
    /// word is split until the report, or the cluster start there.
    fn anchor(&self, display: &DisplaySnapshot, target: usize, floor: usize) -> usize {
        let row_start = self.row_span(display, self.row_of(display, target)).0;
        let back = display
            .cluster_at(floor.max(target.saturating_sub(ANCHOR_BACK)))
            .0;
        if row_start >= back || row_start < floor {
            return row_start;
        }
        let limit = target.min(back.saturating_add(SPACE_SEARCH));
        let (mut offset, mut spaces) = (back, false);
        for piece in display.text_in(back..limit) {
            for (index, byte) in piece.bytes().enumerate() {
                if byte == b' ' {
                    spaces = true;
                } else if spaces {
                    return display.cluster_at(offset + index).0;
                }
            }
            offset += piece.len();
        }
        // The last space before the bound, but not before the row or the provisional rows.
        let from = back.saturating_sub(SPACE_SEARCH).max(row_start).max(floor);
        let (mut offset, mut after) = (from, None);
        for piece in display.text_in(from..back) {
            for (index, byte) in piece.bytes().enumerate() {
                if byte == b' ' {
                    after = Some(offset + index + 1);
                }
            }
            offset += piece.len();
        }
        after.map_or(back, |after| display.cluster_at(after).0)
    }
}

impl WrapMap {
    /// Wraps the provisional rows from the one that holds `at` to `rows` rows past it at the
    /// window's width, those not yet wrapped here, within what is left of the command's
    /// budget. Rows wrapped before `at` from the same anchor come too. Returns whether any
    /// row changed, after which a top or a cursor's row must be found again.
    pub(crate) fn realize(&mut self, display: &DisplaySnapshot, at: usize, rows: usize) -> bool {
        let mut changed = false;
        for _ in 0..PASSES {
            if !self.wrap.wraps() || self.budget == 0 || self.unreal.is_empty() {
                break;
            }
            let at = at.min(display.len());
            let first = self.wrap.row_of(display, at);
            let last = first.saturating_add(rows).min(self.wrap.row_count() - 1);
            let start = self.wrap.row_span(display, first).0;
            let end = self.wrap.row_span(display, last).1.max(start + 1);
            let index = self.unreal.partition_point(|range| range.end <= start);
            let Some(unreal) = (self.unreal.get(index))
                .filter(|range| range.start < end)
                .cloned()
            else {
                break;
            };
            let target = at.max(unreal.start).min(display.len());
            let before = self.wrap.row_of(display, target).saturating_sub(first);
            let anchor = self.wrap.anchor(display, target, unreal.start);
            let wanted = (target, rows.saturating_sub(before), unreal.end);
            let (wrap, reached, join) =
                self.wrap
                    .realized(display, anchor, wanted, &mut self.budget);
            self.wrap = wrap;
            changed = true;
            remove(&mut self.unreal, anchor..reached.max(anchor));
            if join > reached {
                self.want_range(reached..join);
            }
            if reached <= anchor {
                break;
            }
        }
        changed
    }
}

/// `ranges`, sorted and apart, less `cut`.
fn remove(ranges: &mut Vec<Range<usize>>, cut: Range<usize>) {
    if cut.is_empty() {
        return;
    }
    let mut kept = Vec::with_capacity(ranges.len() + 1);
    for range in ranges.drain(..) {
        if range.end <= cut.start || range.start >= cut.end {
            kept.push(range);
            continue;
        }
        if range.start < cut.start {
            kept.push(range.start..cut.start);
        }
        if range.end > cut.end {
            kept.push(cut.end..range.end);
        }
    }
    *ranges = kept;
}
