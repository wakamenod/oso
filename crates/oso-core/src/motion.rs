//! Cursor motion over the display model (ADR 0030, ADR 0040).
//!
//! Must not depend on: `buffer`, `window` or `editor`; it works on offsets and
//! selections over a display snapshot.
//!
//! Positions step over display clusters, the units the cell grid draws: a
//! grapheme cluster (`が`, a flag, a ZWJ emoji family), a tab, a `\r\n`
//! line break, a control escape, a boxed invisible character or an invalid
//! byte. So one `C-f` passes what looks like one character, and the cursor
//! never lands inside one. Rows are buffer lines (no soft wrap, ADR 0006).

use crate::display_map::{CellKind, DisplayPoint, DisplaySnapshot};
use crate::selection::{Goal, Selection};
use crate::sum_tree::Bias;

/// A motion reached the start or the end of the buffer before it was done.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Edge {
    /// "Beginning of buffer".
    Start,
    /// "End of buffer".
    End,
}

impl Edge {
    /// The message Emacs shows.
    pub fn message(self) -> &'static str {
        match self {
            Self::Start => "Beginning of buffer",
            Self::End => "End of buffer",
        }
    }
}

/// The offset after the display cluster at `offset`, or the start of the
/// next line at a line end. `None` at the end of the buffer.
pub fn next_cluster(display: &DisplaySnapshot, offset: usize) -> Option<usize> {
    let point = display.offset_to_display_point(offset);
    if let Some(cluster) = display.clusters(point).next() {
        return Some(cluster.offset + cluster.text.len());
    }
    (point.row + 1 < display.row_count()).then(|| line_start(display, point.row + 1))
}

/// The offset of the display cluster before `offset`, or the end of the
/// previous line at a line start. `None` at the start of the buffer.
pub fn prev_cluster(display: &DisplaySnapshot, offset: usize) -> Option<usize> {
    let point = display.offset_to_display_point(offset);
    if point.column == 0 {
        return (point.row > 0).then(|| line_end(display, point.row - 1));
    }
    let before = DisplayPoint::new(point.row, point.column - 1);
    let cluster = display.clusters(before).next()?;
    Some(cluster.offset)
}

/// The offset where `row` starts; the end of the buffer past the last row.
pub fn line_start(display: &DisplaySnapshot, row: usize) -> usize {
    display.display_point_to_offset(DisplayPoint::new(row, 0), Bias::Left)
}

/// The offset where `row` ends, before its line break; the end of the
/// buffer past the last row.
pub fn line_end(display: &DisplaySnapshot, row: usize) -> usize {
    display.display_point_to_offset(DisplayPoint::new(row, usize::MAX), Bias::Left)
}

/// The end of the buffer.
pub fn buffer_end(display: &DisplaySnapshot) -> usize {
    let last = display.row_count() - 1;
    line_end(display, last)
}

/// The cells the cursor box covers at `point`: the cluster's, or 1 at a
/// line end. A tab gets 1 cell, as Emacs draws it with `x-stretch-cursor`
/// off.
pub fn cursor_cells(display: &DisplaySnapshot, point: DisplayPoint) -> usize {
    match display.clusters(point).next() {
        Some(cluster) if cluster.kind != CellKind::Tab => cluster.cells.max(1),
        _ => 1,
    }
}

/// Moves `selection` by `n` display clusters, backward if `n` is
/// negative. Stops at the buffer's edge and reports it.
pub fn move_clusters(
    display: &DisplaySnapshot,
    selection: &mut Selection,
    n: isize,
) -> Result<(), Edge> {
    selection.goal = None;
    let step = if n < 0 { prev_cluster } else { next_cluster };
    for _ in 0..n.unsigned_abs() {
        match step(display, selection.cursor) {
            Some(offset) => selection.cursor = offset,
            None if n < 0 => return Err(Edge::Start),
            None => return Err(Edge::End),
        }
    }
    Ok(())
}

/// Moves `selection` `n` lines down (up if negative), aiming at its goal
/// column. The first vertical move sets the goal: the cursor's display
/// column, or [`Goal::LineEnd`] at a line end (`track-eol`). A column in
/// the middle of a wide character or a tab lands at its start.
///
/// Past the last line the cursor goes to the end of the buffer, and before
/// the first to its start, as Emacs's `next-line` does, and the edge is
/// reported. The goal is kept for the next vertical move.
pub fn move_lines(
    display: &DisplaySnapshot,
    selection: &mut Selection,
    n: isize,
) -> Result<(), Edge> {
    let point = display.offset_to_display_point(selection.cursor);
    let goal = selection.goal.unwrap_or_else(|| {
        if selection.cursor == line_end(display, point.row) {
            Goal::LineEnd
        } else {
            Goal::Column(point.column)
        }
    });
    selection.goal = Some(goal);
    let last = display.row_count() - 1;
    let target = point.row.saturating_add_signed(n);
    if n < 0 && point.row < n.unsigned_abs() {
        selection.cursor = 0;
        return Err(Edge::Start);
    }
    if target > last {
        selection.cursor = buffer_end(display);
        return Err(Edge::End);
    }
    selection.cursor = match goal {
        Goal::LineEnd => line_end(display, target),
        Goal::Column(column) => {
            display.display_point_to_offset(DisplayPoint::new(target, column), Bias::Left)
        }
    };
    Ok(())
}

/// Moves `selection` to the start of its line, after moving `n - 1` lines
/// down first (up if `n - 1` is negative), as Emacs's
/// `move-beginning-of-line` does.
pub fn move_line_start(display: &DisplaySnapshot, selection: &mut Selection, n: isize) {
    let row = target_row(display, selection.cursor, n);
    *selection = Selection::at(line_start(display, row));
}

/// Moves `selection` to the end of its line, after moving `n - 1` lines
/// down first (up if negative).
pub fn move_line_end(display: &DisplaySnapshot, selection: &mut Selection, n: isize) {
    let row = target_row(display, selection.cursor, n);
    *selection = Selection::at(line_end(display, row));
}

/// The row `n - 1` lines from the cursor's, clamped to the buffer.
fn target_row(display: &DisplaySnapshot, offset: usize, n: isize) -> usize {
    let row = display.offset_to_display_point(offset).row;
    row.saturating_add_signed(n.saturating_sub(1))
        .min(display.row_count() - 1)
}

/// The start of the line at `tenths` tenths of the buffer, for `M-<` and
/// `M->` with a numeric argument: Emacs goes there and on to the start of
/// the next line.
pub fn line_after_fraction(display: &DisplaySnapshot, len: usize, tenths: usize) -> usize {
    let offset = (len as u128 * tenths.min(10) as u128 / 10) as usize;
    let row = display.offset_to_display_point(offset).row;
    line_start(display, row + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display_map::{DisplayMap, TabWidth};
    use crate::rope::Rope;
    use proptest::prelude::*;

    fn display(text: &str) -> DisplaySnapshot {
        DisplayMap::new(&Rope::from(text), TabWidth::default()).snapshot()
    }

    /// Every cursor stop going forward from the start.
    fn stops(display: &DisplaySnapshot) -> Vec<usize> {
        let mut stops = vec![0];
        while let Some(next) = next_cluster(display, *stops.last().unwrap()) {
            stops.push(next);
        }
        stops
    }

    #[test]
    fn clusters_are_single_stops() {
        let text = "aが👨‍👩‍👧🇯🇵\tb\r\n\u{202e}x";
        let d = display(text);
        let stops = stops(&d);
        let pieces: Vec<&str> = stops.windows(2).map(|w| &text[w[0]..w[1]]).collect();
        assert_eq!(
            pieces,
            ["a", "が", "👨‍👩‍👧", "🇯🇵", "\t", "b", "\r\n", "\u{202e}", "x"]
        );
    }

    #[test]
    fn edges_have_the_messages_emacs_shows() {
        assert_eq!(Edge::Start.message(), "Beginning of buffer");
        assert_eq!(Edge::End.message(), "End of buffer");
    }

    #[test]
    fn cluster_motion_counts_and_stops_at_the_edges() {
        let d = display("a日b");
        let mut s = Selection::at(0);
        s.goal = Some(Goal::Column(7));
        assert_eq!(move_clusters(&d, &mut s, 2), Ok(()));
        assert_eq!((s.cursor, s.goal), (4, None));
        assert_eq!(move_clusters(&d, &mut s, -1), Ok(()));
        assert_eq!(s.cursor, 1);
        assert_eq!(move_clusters(&d, &mut s, 0), Ok(()));
        assert_eq!(s.cursor, 1);
        // Goes as far as it can before reporting the edge.
        assert_eq!(move_clusters(&d, &mut s, 9), Err(Edge::End));
        assert_eq!(s.cursor, 5);
        assert_eq!(move_clusters(&d, &mut s, -9), Err(Edge::Start));
        assert_eq!(s.cursor, 0);
    }

    #[test]
    fn line_motion_keeps_the_goal_column_in_cells() {
        // Column 4: after "ab日" on line 0; inside "日本" on line 1.
        let d = display("ab日cd\n日本語\nx\nabcdefgh");
        let mut s = Selection::at(5); // "c", column 4
        move_lines(&d, &mut s, 1).unwrap();
        assert_eq!(d.offset_to_display_point(s.cursor), DisplayPoint::new(1, 4));
        move_lines(&d, &mut s, 1).unwrap();
        assert_eq!(d.offset_to_display_point(s.cursor), DisplayPoint::new(2, 1));
        move_lines(&d, &mut s, 1).unwrap();
        assert_eq!(d.offset_to_display_point(s.cursor), DisplayPoint::new(3, 4));
        assert_eq!(s.goal, Some(Goal::Column(4)));
    }

    #[test]
    fn a_goal_inside_a_wide_character_lands_at_its_start() {
        let d = display("abc\n日本");
        let mut s = Selection::at(1); // column 1
        move_lines(&d, &mut s, 1).unwrap();
        assert_eq!(d.offset_to_display_point(s.cursor), DisplayPoint::new(1, 0));
    }

    #[test]
    fn line_motion_from_a_line_end_tracks_line_ends() {
        let d = display("abc\nabcdef\nab");
        let mut s = Selection::at(3);
        move_lines(&d, &mut s, 1).unwrap();
        assert_eq!(s.cursor, line_end(&d, 1));
        assert_eq!(s.goal, Some(Goal::LineEnd));
        move_lines(&d, &mut s, 1).unwrap();
        assert_eq!(s.cursor, line_end(&d, 2));
    }

    #[test]
    fn line_motion_past_the_edges_goes_to_the_buffer_ends() {
        let d = display("abc\nde");
        let mut s = Selection::at(5);
        assert_eq!(move_lines(&d, &mut s, 1), Err(Edge::End));
        assert_eq!(s.cursor, 6);
        let mut s = Selection::at(2);
        assert_eq!(move_lines(&d, &mut s, -1), Err(Edge::Start));
        assert_eq!(s.cursor, 0);
        let mut s = Selection::at(2);
        assert_eq!(move_lines(&d, &mut s, isize::MIN), Err(Edge::Start));
        assert_eq!(move_lines(&d, &mut s, isize::MAX), Err(Edge::End));
    }

    #[test]
    fn line_motion_to_the_first_line_is_not_past_the_edge() {
        let d = display("abc\nde");
        let mut s = Selection::at(5);
        assert_eq!(move_lines(&d, &mut s, -1), Ok(()));
        assert_eq!(s.cursor, 1);
    }

    #[test]
    fn line_start_and_end_take_a_count() {
        let d = display("ab\r\ncd\nef");
        let mut s = Selection::at(5);
        move_line_end(&d, &mut s, 1);
        assert_eq!(s.cursor, 6);
        move_line_start(&d, &mut s, 1);
        assert_eq!(s.cursor, 4);
        move_line_end(&d, &mut s, 2);
        assert_eq!(s.cursor, 9);
        move_line_start(&d, &mut s, 0);
        assert_eq!(s.cursor, 4);
        move_line_end(&d, &mut s, isize::MIN);
        assert_eq!(s.cursor, 2, "before the CRLF");
        move_line_start(&d, &mut s, isize::MAX);
        assert_eq!(s.cursor, 7);
    }

    #[test]
    fn cursor_cells_follow_the_cluster() {
        let d = display("a日\tb");
        assert_eq!(cursor_cells(&d, DisplayPoint::new(0, 0)), 1);
        assert_eq!(cursor_cells(&d, DisplayPoint::new(0, 1)), 2);
        assert_eq!(cursor_cells(&d, DisplayPoint::new(0, 3)), 1, "a tab");
        assert_eq!(cursor_cells(&d, DisplayPoint::new(0, 9)), 1, "line end");
    }

    #[test]
    fn fractions_go_to_the_next_line_start() {
        let d = display("0123456789\n0123456789\n");
        assert_eq!(line_after_fraction(&d, 22, 0), 11);
        assert_eq!(line_after_fraction(&d, 22, 5), 22);
        assert_eq!(line_after_fraction(&d, 22, 10), 22);
        let d = display("0123456789\n0123456789\n0123456789\n");
        assert_eq!(line_after_fraction(&d, 33, 5), 22, "the second line's end");
    }

    proptest! {
        /// Forward and backward stops are the same, each one maps to a
        /// display point and back, and no stop is inside a character.
        #[test]
        fn prop_stops_are_consistent(text in crate::rope::tests::text_strategy(40)) {
            let d = display(&text);
            let forward = stops(&d);
            let mut backward = vec![*forward.last().unwrap()];
            while let Some(prev) = prev_cluster(&d, *backward.last().unwrap()) {
                backward.push(prev);
            }
            backward.reverse();
            prop_assert_eq!(&forward, &backward);
            prop_assert_eq!(*forward.last().unwrap(), text.len());
            for &stop in &forward {
                prop_assert!(text.is_char_boundary(stop));
                let point = d.offset_to_display_point(stop);
                prop_assert_eq!(d.display_point_to_offset(point, Bias::Left), stop);
            }
        }

        /// Vertical motion always lands on a stop of the target row.
        #[test]
        fn prop_line_motion_lands_on_stops(text in crate::rope::tests::text_strategy(40), start in 0usize..200, n in -5isize..5) {
            let d = display(&text);
            let all = stops(&d);
            let mut s = Selection::at(all[start % all.len()]);
            let _ = move_lines(&d, &mut s, n);
            prop_assert!(all.contains(&s.cursor));
        }
    }
}
