//! The faces a buffer's plain text takes from its highlights (ADR 0111). They are picked after
//! shaping, glyph by glyph, as the glyph instances are built, so a report shapes nothing again.
//! The rows of cells they give way to, [`CellRows`], also say where text is faded (ADR 0138).
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

/// Frame cells of a text pane, each a frame row and its columns, sorted, and those of a row that
/// meet merged, so that a row's glyphs walk them once. Kept from frame to frame for its room.
/// One holds where text keeps [`Face::Text`]'s colour, another where it is faded (ADR 0138).
#[derive(Default)]
pub(super) struct CellRows(Vec<(i64, Range<i64>)>);

impl CellRows {
    /// Takes the cells of `view`'s fills, but those of [`SYNTAX_FILLS`], and of its flash, on
    /// the rows of `region`: where text keeps [`Face::Text`]'s colour.
    pub(super) fn read(&mut self, view: &TextView<'_>, region: CellRect) {
        let fills = view
            .fills
            .iter()
            .filter(|(face, cells)| !cells.is_empty() && !SYNTAX_FILLS.contains(face));
        let flash = view.flash.as_ref().map(|flash| &flash.cells);
        self.read_cells(
            fills.flat_map(|(_, cells)| cells.iter()).chain(flash),
            region,
        );
    }

    /// Takes `cells`, on the rows of `region`.
    pub(super) fn read_cells<'a>(
        &mut self,
        cells: impl Iterator<Item = &'a CellRect>,
        region: CellRect,
    ) {
        self.0.clear();
        for cells in cells {
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

    /// Whether it holds no cell.
    pub(super) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The columns of frame row `row`, to walk.
    pub(super) fn on(&self, row: i64) -> RowCells<'_> {
        let start = self.0.partition_point(|(at, _)| *at < row);
        let end = self.0.partition_point(|(at, _)| *at <= row);
        RowCells(&self.0[start..end])
    }
}

/// One frame row's columns of [`CellRows`], asked about in rising columns.
#[derive(Clone, Copy, Default)]
pub(super) struct RowCells<'a>(&'a [(i64, Range<i64>)]);

impl RowCells<'_> {
    /// Whether any of the frame's columns `cells` is in them. Columns left of `cells` are done
    /// with, as glyphs come in rising columns.
    pub(super) fn meets(&mut self, cells: &Range<i64>) -> bool {
        while let Some(((_, columns), rest)) = self.0.split_first()
            && columns.end <= cells.start
        {
            self.0 = rest;
        }
        (self.0.first()).is_some_and(|(_, columns)| columns.start < cells.end)
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
    plain: RowCells<'a>,
}

impl<'a> RowFaces<'a> {
    /// The faces of a row of `clusters`, on frame row `row`. Each row seeks in the highlights,
    /// rather than one walk going on from the row before, so that the part of a long line out
    /// of view costs nothing; the seek's cursor is the one allocation a row.
    pub(super) fn new(
        highlights: &'a Highlights,
        clusters: &'a [(usize, usize)],
        plain: &'a CellRows,
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
        if self.plain.meets(&cells) {
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
