//! The faces a buffer's plain text takes from its highlights (ADR 0111). They are picked after
//! shaping, glyph by glyph, as the glyph instances are built, so a report shapes nothing again.
//!
//! Must not depend on: `capture`, `paint` or a font backend.
//!
//! - A row costs one seek in the highlights, at its first cluster, and a step per run.
//! - Only a glyph of [`Face::Text`] takes a syntax face: an escape, a box, a whitespace mark,
//!   the wrap mark, the preedit and the glyph under the cursor keep theirs. A ligature takes
//!   its first cluster's.
//! - On a fill other than the current line's band, and under the flash, text keeps
//!   [`Face::Text`]'s colour, which meets its floor on each of them: a syntax face is checked
//!   on the background and the band only (the author's choice).

use std::ops::Range;

use oso_core::frame::CellRect;
use oso_core::syntax::{Highlight, Highlights, Runs};

use crate::pane::{Face, TextView};
use crate::theme::SYNTAX_FILLS;

/// The frame's cells of a text pane where text keeps [`Face::Text`]'s colour: each a frame row
/// and its columns, sorted, and those of a row that meet merged, so that a row's glyphs walk
/// them once. Kept from frame to frame for its room.
#[derive(Default)]
pub(super) struct Plain(Vec<(i64, Range<i64>)>);

impl Plain {
    /// Takes the cells of `view`'s fills, but those of [`SYNTAX_FILLS`], and of its flash, on
    /// the rows of `region`.
    pub(super) fn read(&mut self, view: &TextView<'_>, region: CellRect) {
        self.0.clear();
        let fills = view
            .fills
            .iter()
            .filter(|(face, _)| !SYNTAX_FILLS.contains(face));
        let flash = view.flash.as_ref().map(|flash| &flash.cells);
        for cells in fills.flat_map(|(_, cells)| cells.iter()).chain(flash) {
            let columns = cells.column as i64..cells.column.saturating_add(cells.columns) as i64;
            let rows = cells.row.max(region.row)
                ..(cells.row.saturating_add(cells.rows)).min(region.row + region.rows);
            self.0.extend(rows.map(|row| (row as i64, columns.clone())));
        }
        self.0
            .sort_unstable_by_key(|(row, columns)| (*row, columns.start));
        // Merged in place: the isearch matches, the regions and the flash may meet.
        let mut kept: usize = 0;
        for at in 0..self.0.len() {
            let (row, columns) = self.0[at].clone();
            match kept.checked_sub(1).map(|last| &mut self.0[last]) {
                Some((last_row, last)) if *last_row == row && columns.start <= last.end => {
                    last.end = last.end.max(columns.end);
                }
                _ => {
                    self.0[kept] = (row, columns);
                    kept += 1;
                }
            }
        }
        self.0.truncate(kept);
    }

    /// The columns of frame row `row`.
    fn on(&self, row: i64) -> &[(i64, Range<i64>)] {
        let start = self.0.partition_point(|(at, _)| *at < row);
        let end = self.0.partition_point(|(at, _)| *at <= row);
        &self.0[start..end]
    }
}

/// The faces of one row's glyphs, asked for in the order of their columns.
pub(super) struct RowFaces<'a> {
    /// The row's clusters: each one's first column, from the row's base, and its offset in
    /// the buffer.
    clusters: &'a [(usize, usize)],
    /// The cluster the last glyph asked about starts in.
    next: usize,
    runs: Runs<'a>,
    run: Option<(Range<usize>, Option<Highlight>)>,
    /// The row's plain columns from the first that may still meet a glyph.
    plain: &'a [(i64, Range<i64>)],
}

impl<'a> RowFaces<'a> {
    /// The faces of a row of `clusters`, on frame row `row`. Each row seeks in the highlights,
    /// rather than one walk going on from the row before, so that the part of a long line out
    /// of view costs nothing; the seek's cursor is the one allocation a row.
    pub(super) fn new(
        highlights: &'a Highlights,
        clusters: &'a [(usize, usize)],
        plain: &'a Plain,
        row: i64,
    ) -> Self {
        let end = highlights.len();
        let start = clusters.first().map_or(end, |&(_, offset)| offset);
        let mut runs = highlights.runs(start..end);
        let run = runs.next();
        Self {
            clusters,
            next: 0,
            runs,
            run,
            plain: plain.on(row),
        }
    }

    /// The face of a glyph of [`Face::Text`] at `column` of the row, from its base, which
    /// takes the frame's columns `cells`.
    pub(super) fn face(&mut self, column: usize, cells: Range<i64>) -> Face {
        // Glyphs come in rising columns, so plain columns left of this one are done with.
        while let Some(((_, plain), rest)) = self.plain.split_first()
            && plain.end <= cells.start
        {
            self.plain = rest;
        }
        if self
            .plain
            .first()
            .is_some_and(|(_, plain)| plain.start < cells.end)
        {
            return Face::Text;
        }
        while self
            .clusters
            .get(self.next + 1)
            .is_some_and(|&(start, _)| start <= column)
        {
            self.next += 1;
        }
        let Some(&(start, offset)) = self.clusters.get(self.next) else {
            return Face::Text;
        };
        if start > column {
            return Face::Text;
        }
        while self
            .run
            .as_ref()
            .is_some_and(|(range, _)| range.end <= offset)
        {
            self.run = self.runs.next();
        }
        match &self.run {
            Some((range, Some(highlight))) if range.start <= offset => {
                Face::of_highlight(*highlight)
            }
            _ => Face::Text,
        }
    }
}

#[cfg(test)]
mod tests;
