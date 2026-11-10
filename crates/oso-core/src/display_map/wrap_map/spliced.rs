//! The rows of a logical line laid out again with an input method's preedit in it, as if it
//! were inserted (ADR 0100): so a preedit at a row's end wraps, and one that starts with `。`
//! pulls the row before it as committing it will.
//!
//! Must not depend on: windows or the renderer; the caller says where the preedit is.
//!
//! The layout starts where an edit at the cursor would start its rewrap, and reads the line's
//! clusters with the preedit's spliced in, as `row_clusters` draws them, a row at a time, for
//! as many rows as the caller asks. Nothing is kept: the window's rows stay as they are.

use super::breaks::{Cluster, Rows};
use super::{WrapSnapshot, buffer_end, line_end};
use crate::display_map::{DisplayCluster, DisplayPoint, DisplaySnapshot, Preedit};

/// One row of a line laid out with a preedit in it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct SplicedRow {
    /// Where it starts and where the next row starts, in the buffer. A row that starts in the
    /// preedit starts at the cursor.
    pub(crate) start: usize,
    pub(crate) end: usize,
    /// The cell column on its line where it starts, with the preedit's cells in.
    pub(crate) column: usize,
    /// Whether its line goes on in the next row.
    pub(crate) continues: bool,
}

impl WrapSnapshot {
    /// The rows of the logical line that holds `cursor`, a cluster start, with `preedit`
    /// spliced in there, into `out`, emptied first: from the row an edit at the cursor would
    /// start its rewrap at, until the line ends, `out` holds `most` rows, or a row holds the
    /// column `until`. Returns that row's index among these rows; nothing when rows do not
    /// wrap. Each row reads at most a row's clusters and the rules' lookahead.
    pub(crate) fn spliced_rows(
        &self,
        display: &DisplaySnapshot,
        (cursor, preedit): (usize, &Preedit),
        (most, until): (usize, usize),
        out: &mut Vec<SplicedRow>,
    ) -> usize {
        out.clear();
        let from = self.rewrap_start(display, cursor);
        let first = self.row_of(display, from);
        if !self.wraps() || most == 0 {
            return first;
        }
        let start = display.offset_to_display_point(from);
        let at = display.offset_to_display_point(cursor);
        let len = preedit.text().len();
        // Offsets past the cursor move by the preedit's length, and its clusters take the
        // offsets between, so that the rules see one text.
        let before = display
            .clusters(start)
            .take_while(move |cluster| cluster.offset < cursor);
        let composed = preedit
            .clusters_from(at.column, 0)
            .map(move |cluster| DisplayCluster {
                offset: cursor + cluster.offset,
                ..cluster
            });
        let after = display
            .clusters(DisplayPoint::new(at.row, at.column))
            .map(move |cluster| DisplayCluster {
                offset: cluster.offset + len,
                column: cluster.column + preedit.cells(),
                ..cluster
            });
        let clusters = before.chain(composed).chain(after).map(Cluster::of);
        let line_end = line_end(display, at.row, buffer_end(display)) + len;
        let in_buffer = |offset: usize| {
            if offset <= cursor {
                offset
            } else {
                offset.saturating_sub(len).max(cursor)
            }
        };
        let (mut row_start, mut column) = (from, start.column);
        for row in Rows::new(clusters, self.width, line_end) {
            out.push(SplicedRow {
                start: in_buffer(row_start),
                end: in_buffer(row.end),
                column,
                continues: row.continues,
            });
            (row_start, column) = (row.end, row.next_column);
            if !row.continues || out.len() == most || column > until {
                break;
            }
        }
        first
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display_map::{DisplayMap, TabWidth};
    use crate::rope::Rope;
    use proptest::prelude::*;

    fn snapshot(text: &str) -> DisplaySnapshot {
        DisplayMap::new(&Rope::from(text), TabWidth::default()).snapshot()
    }

    /// The rows of `cursor`'s line with `composed` spliced in, as (column, continues).
    fn spliced(text: &str, cursor: usize, composed: &str, columns: usize) -> Vec<(usize, bool)> {
        let display = snapshot(text);
        let wrap = WrapSnapshot::new(&display, columns);
        let preedit = Preedit::new(composed.into(), None);
        let mut out = Vec::new();
        let first = wrap.spliced_rows(
            &display,
            (cursor, &preedit),
            (usize::MAX, usize::MAX),
            &mut out,
        );
        // It starts on a row the window has.
        assert_eq!(wrap.row(&display, first).start, out[0].start);
        out.iter().map(|row| (row.column, row.continues)).collect()
    }

    /// The rows of the same line once `composed` is inserted, from the same row on.
    fn inserted(text: &str, cursor: usize, composed: &str, columns: usize) -> Vec<(usize, bool)> {
        let mut whole = text.to_owned();
        whole.insert_str(cursor, composed);
        let display = snapshot(&whole);
        let wrap = WrapSnapshot::new(&display, columns);
        let before = snapshot(text);
        let from = WrapSnapshot::new(&before, columns).rewrap_start(&before, cursor);
        let line = display.offset_to_display_point(cursor).row;
        (wrap.row_of(&display, from)..wrap.row_count())
            .map(|row| wrap.row(&display, row))
            .take_while(|row| row.line == line)
            .map(|row| (row.column, row.continues))
            .collect()
    }

    #[test]
    fn a_preedit_at_a_rows_end_wraps_as_if_inserted() {
        // 11 columns, 10 for text.
        let text = "aaaa bbbbb cc";
        assert_eq!(
            spliced(text, 10, "xyz", 11),
            [(0, true), (5, true), (14, false)]
        );
        assert_eq!(spliced(text, 10, "xyz", 11), inserted(text, 10, "xyz", 11));
    }

    #[test]
    fn a_closing_mark_at_a_rows_start_pulls_the_row_before_it() {
        // Five wide characters fill a row of 10 cells; `。` may not start the next.
        let text = "あいうえおかき";
        let rows = spliced(text, 15, "。", 11);
        assert_eq!(rows, inserted(text, 15, "。", 11));
        assert_eq!(rows[1], (8, false), "お moves down with 。");
    }

    #[test]
    fn rows_after_the_line_are_not_laid_out_and_the_count_is_kept() {
        let display = snapshot("aaaa bbbb cccc dddd\nnext");
        let wrap = WrapSnapshot::new(&display, 11);
        let preedit = Preedit::new("xy".into(), None);
        let mut out = Vec::new();
        wrap.spliced_rows(&display, (0, &preedit), (2, usize::MAX), &mut out);
        assert_eq!(out.len(), 2);
        wrap.spliced_rows(&display, (0, &preedit), (usize::MAX, 3), &mut out);
        assert_eq!(out.len(), 1, "the first row holds column 3");
        wrap.spliced_rows(&display, (0, &preedit), (usize::MAX, usize::MAX), &mut out);
        assert_eq!(out.len(), 3);
        assert!(!out.last().unwrap().continues);
        assert!(out.iter().all(|row| row.end <= 20));
    }

    #[test]
    fn a_row_that_starts_in_the_preedit_starts_at_the_cursor() {
        let display = snapshot("ab");
        let wrap = WrapSnapshot::new(&display, 11);
        let preedit = Preedit::new("0123456789abc".into(), None);
        let mut out = Vec::new();
        wrap.spliced_rows(&display, (1, &preedit), (usize::MAX, usize::MAX), &mut out);
        let starts: Vec<_> = out.iter().map(|row| (row.start, row.column)).collect();
        assert_eq!(starts, [(0, 0), (1, 10)]);
        assert_eq!(out[1].end, 2);
    }

    #[test]
    fn nothing_is_laid_out_while_rows_do_not_wrap() {
        let display = snapshot("abc");
        let wrap = WrapSnapshot::unwrapped(&display);
        let mut out = Vec::new();
        let preedit = Preedit::new("x".into(), None);
        wrap.spliced_rows(&display, (1, &preedit), (4, usize::MAX), &mut out);
        assert!(out.is_empty());
    }

    proptest! {
        /// Whatever the text, the cursor and the preedit, the rows are those of the text with
        /// the preedit inserted.
        #[test]
        fn prop_a_preedit_wraps_as_if_inserted(
            text in "[a あ。、「（\n]{0,40}",
            composed in "[b い。」）]{1,12}",
            at in 0usize..64,
            columns in 10usize..16,
        ) {
            let cursor = text.floor_char_boundary(at.min(text.len()));
            prop_assert_eq!(
                spliced(&text, cursor, &composed, columns),
                inserted(&text, cursor, &composed, columns),
            );
        }
    }
}
