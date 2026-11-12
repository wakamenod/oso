//! Cursor motion over the display model (ADR 0030, ADR 0040).
//!
//! Must not depend on: `buffer`, `window` or `editor`; it works on offsets and
//! selections over a display snapshot.
//!
//! Positions step over display clusters, the units the cell grid draws: a
//! grapheme cluster (`が`, a flag, a ZWJ emoji family), a tab, a `\r\n`
//! line break, a control escape, a boxed invisible character, a boxed character
//! past a cluster's limits (ADR 0054) or an invalid byte. So one `C-f` passes what
//! looks like one character, and the cursor never lands inside one. Rows are
//! buffer lines (no soft wrap, ADR 0006).
//!
//! A motion that reads text cluster by cluster is limited, so that a count or a long
//! run of one kind of character cannot stall a key ([`WalkBudget`], ADR 0058).

use crate::display_map::{CellKind, DisplayCluster, DisplayPoint, DisplaySnapshot, Preedit};
use crate::selection::{Goal, Selection};
use crate::sum_tree::Bias;
use crate::syntax::Highlights;

mod code_end;

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

/// What one command may read, in bytes read in order, at about 8 to 13 ns each:
/// about 50 ms at most, under the 100 ms that counts as a freeze. It holds across
/// all its cursors and its count, but for word motion at several cursors, which has
/// [`MAX_CURSORS_WALK`]. Bytes, not clusters: a cluster can hold many
/// characters, and the cost per byte stays about the same.
const MAX_COMMAND_WALK: usize = 1 << 22;

/// What one command at several cursors may read, across all of them: about 7 ms at the
/// slowest bytes word motion reads, about 80 KB a millisecond, so that a key at many
/// cursors on long lines with no word boundary stays within a frame. The cursors share it
/// in order, and a cursor it runs out on stops where it got to (ADR 0101).
const MAX_CURSORS_WALK: usize = 1 << 19;

/// The seeks to where it starts that each cursor of a command at several gets apart from
/// [`MAX_CURSORS_WALK`]: as many as finding the word around a cursor takes, one forward
/// and two back. Charged as bytes they would spend the bytes of a key at about a thousand
/// cursors on any text, though one costs far less on most (ADR 0101).
const ENTRY_SEEKS: usize = 3;

/// What finding one stop may read, such as the end of a word: a key stays within
/// its budget on a long line with no word boundary, at about 2 ms at most. Past it
/// the motion stops where it got to, and the next key goes on from there.
pub(crate) const MAX_STOP_WALK: usize = 1 << 17;

/// What going to another place in the text costs, in bytes read in order: it
/// searches the display model and scans part of a chunk.
pub(crate) const SEEK_COST: usize = 512;

/// The columns read at a time when looking backward along a row. Clusters read
/// only forward, so a backward search reads this much and moves back by it.
pub(crate) const BACK_WINDOW: usize = 128;

/// What a command signals when its count asks for too much work.
pub const TOO_MUCH: &str = "Count too large";

/// What a command may still read (ADR 0058). A command makes one and passes it to
/// each cursor's motion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WalkBudget {
    left: usize,
    ran_out: bool,
    /// Entry seeks left that take nothing from `left`, shared by the cursors.
    seeks: usize,
}

impl WalkBudget {
    /// The budget of a whole command.
    pub const fn new() -> Self {
        Self {
            left: MAX_COMMAND_WALK,
            ran_out: false,
            seeks: 0,
        }
    }

    /// The budget of a whole command at `cursors` cursors: [`WalkBudget::new`]'s for one,
    /// and for several a frame's of bytes, which they share, with a few entry seeks for
    /// each apart from it (ADR 0101).
    pub const fn for_cursors(cursors: usize) -> Self {
        if cursors > 1 {
            Self {
                left: MAX_CURSORS_WALK,
                ran_out: false,
                seeks: cursors.saturating_mul(ENTRY_SEEKS),
            }
        } else {
            Self::new()
        }
    }

    /// Takes `bytes` from the budget. If it does not hold that many, it is left
    /// empty and has run out, and this returns false. Taking all it holds is fine.
    pub(crate) fn spend(&mut self, bytes: usize) -> bool {
        let enough = bytes <= self.left;
        self.left = if enough { self.left - bytes } else { 0 };
        self.ran_out |= !enough;
        enough
    }

    /// Whether it ran out, so that a motion stopped short.
    pub const fn is_spent(self) -> bool {
        self.ran_out
    }

    /// What it still holds.
    pub(crate) const fn left(self) -> usize {
        self.left
    }

    /// A budget of `bytes`, for a command that needs less than a whole one.
    pub(crate) const fn with_limit(bytes: usize) -> Self {
        Self {
            left: bytes,
            ran_out: false,
            seeks: 0,
        }
    }

    /// The reads of one stop.
    pub(crate) fn stop(&mut self) -> StopWalk<'_> {
        StopWalk {
            budget: self,
            left: MAX_STOP_WALK,
        }
    }
}

impl Default for WalkBudget {
    fn default() -> Self {
        Self::new()
    }
}

/// What one stop may still read, from what its command may.
pub(crate) struct StopWalk<'a> {
    budget: &'a mut WalkBudget,
    left: usize,
}

impl StopWalk<'_> {
    /// Takes `bytes`, or returns false if the stop or the command has no more.
    pub(crate) fn spend(&mut self, bytes: usize) -> bool {
        let enough = bytes <= self.left && self.budget.spend(bytes);
        self.left = if enough { self.left - bytes } else { 0 };
        enough
    }

    /// Takes a seek to where the stop starts reading: from the command's entry seeks while
    /// it has some, which leave its bytes alone, else as [`SEEK_COST`] bytes. The stop pays
    /// for it either way. A command with no bytes left takes none, as it can read nothing.
    pub(crate) fn enter(&mut self) -> bool {
        if self.budget.seeks == 0 || self.budget.left == 0 || self.left < SEEK_COST {
            return self.spend(SEEK_COST);
        }
        self.budget.seeks -= 1;
        self.left -= SEEK_COST;
        true
    }
}

/// The start of the display cluster at `offset`, where a cursor can be; the
/// end of the buffer past it.
pub fn cluster_start(display: &DisplaySnapshot, offset: usize) -> usize {
    display.cluster_at(offset).0
}

/// The end of the display cluster `offset` is inside, where a cursor can be;
/// `offset` itself if it starts a cluster. A cursor just after typed text goes
/// here when the text joined what follows, so what it typed stays before it.
pub fn cluster_end(display: &DisplaySnapshot, offset: usize) -> usize {
    display.cluster_at(offset).1
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

/// The clusters of `end.row` before `end`, which is at the offset `end_offset`, from
/// up to [`BACK_WINDOW`] columns back. To go on, call it again at the first one.
pub(crate) fn clusters_before(
    display: &DisplaySnapshot,
    end: DisplayPoint,
    end_offset: usize,
) -> impl Iterator<Item = DisplayCluster<'_>> {
    let start = end.column.saturating_sub(BACK_WINDOW);
    display
        .clusters(DisplayPoint::new(end.row, start))
        .take_while(move |cluster| cluster.offset < end_offset)
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

/// Where the cursor box at `point` goes on the row as shown, and the cells it covers,
/// with `preedit` spliced in at `point` (ADR 0069): on the preedit's caret, or else just
/// after the preedit, on what follows it.
pub fn cursor_box(
    display: &DisplaySnapshot,
    point: DisplayPoint,
    preedit: Option<&Preedit>,
) -> (DisplayPoint, usize) {
    let Some(preedit) = preedit else {
        return (point, cursor_cells(display, point));
    };
    match preedit.caret() {
        Some((before, cells)) => (DisplayPoint::new(point.row, point.column + before), cells),
        None => (
            DisplayPoint::new(point.row, point.column + preedit.cells()),
            cursor_cells(display, point),
        ),
    }
}

/// Moves `selection` by `n` display clusters, backward if `n` is negative. Stops at
/// the buffer's edge and reports it, or where the budget ran out. A count past the
/// edge in bytes goes there without reading what lies between.
pub fn move_clusters(
    display: &DisplaySnapshot,
    selection: &mut Selection,
    n: isize,
    budget: &mut WalkBudget,
) -> Result<(), Edge> {
    selection.goal = None;
    let count = n.unsigned_abs();
    if count > 1 {
        let end = buffer_end(display);
        if n < 0 && count > selection.cursor {
            selection.cursor = 0;
            return Err(Edge::Start);
        }
        if n > 0 && count > end.saturating_sub(selection.cursor) {
            selection.cursor = end;
            return Err(Edge::End);
        }
    }
    let walked = match n {
        0 => return Ok(()),
        1 => next_cluster(display, selection.cursor).ok_or(Edge::End),
        -1 => prev_cluster(display, selection.cursor).ok_or(Edge::Start),
        _ if n < 0 => walk_backward(display, selection.cursor, count, budget),
        _ => walk_forward(display, selection.cursor, count, budget),
    };
    match walked {
        Ok(offset) => {
            selection.cursor = offset;
            Ok(())
        }
        Err(edge) => {
            selection.cursor = if edge == Edge::Start {
                0
            } else {
                buffer_end(display)
            };
            Err(edge)
        }
    }
}

/// Where `count` clusters after `offset` ends, reading them in order. Where the
/// budget ran out if it did, and the buffer's edge if it comes first.
fn walk_forward(
    display: &DisplaySnapshot,
    offset: usize,
    mut count: usize,
    budget: &mut WalkBudget,
) -> Result<usize, Edge> {
    if !budget.spend(SEEK_COST) {
        return Ok(offset);
    }
    let mut at = offset;
    let mut point = display.offset_to_display_point(offset);
    loop {
        for cluster in display.clusters(point) {
            if count == 0 || !budget.spend(cluster.text.len()) {
                return Ok(at);
            }
            at = cluster.offset + cluster.text.len();
            count -= 1;
        }
        if count == 0 {
            return Ok(at);
        }
        if point.row + 1 >= display.row_count() {
            return Err(Edge::End);
        }
        if !budget.spend(SEEK_COST) {
            return Ok(at);
        }
        point = DisplayPoint::new(point.row + 1, 0);
        at = line_start(display, point.row);
        count -= 1;
    }
}

/// Where `count` clusters before `offset` starts, reading windows of a row in turn.
/// Where the budget ran out if it did, and the buffer's edge if it comes first.
fn walk_backward(
    display: &DisplaySnapshot,
    offset: usize,
    mut count: usize,
    budget: &mut WalkBudget,
) -> Result<usize, Edge> {
    if !budget.spend(SEEK_COST) {
        return Ok(offset);
    }
    let mut at = offset;
    let mut point = display.offset_to_display_point(offset);
    while count > 0 {
        if point.column == 0 {
            if point.row == 0 {
                return Err(Edge::Start);
            }
            if !budget.spend(SEEK_COST) {
                return Ok(at);
            }
            at = line_end(display, point.row - 1);
            point = display.offset_to_display_point(at);
            count -= 1;
            continue;
        }
        let (mut clusters, mut bytes) = (0, 0);
        let mut first = None;
        for cluster in clusters_before(display, point, at) {
            first.get_or_insert((cluster.offset, cluster.column));
            clusters += 1;
            bytes += cluster.text.len();
        }
        if !budget.spend(bytes + SEEK_COST) {
            return Ok(at);
        }
        let Some((offset, column)) = first else {
            return Ok(at);
        };
        if clusters >= count {
            let mut window = clusters_before(display, point, at);
            let target = window.nth(clusters - count);
            return Ok(target.map_or(at, |cluster| cluster.offset));
        }
        count -= clusters;
        at = offset;
        point = DisplayPoint::new(point.row, column);
    }
    Ok(at)
}

/// Moves `selection` `n` lines down (up if negative), aiming at its goal
/// column. The first vertical move sets the goal: the cursor's display
/// column, or [`Goal::LineEnd`] at the end of a line with text (`track-eol`). An
/// empty line counts only if a command went to its end, which sets that goal, as
/// in Emacs. A column in the middle of a wide character or a tab lands at its
/// start.
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
        if point.column > 0 && selection.cursor == line_end(display, point.row) {
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
    selection.move_to(line_start(display, row));
}

/// Moves `selection` to the end of its line, after moving `n - 1` lines
/// down first (up if negative). A vertical move from there stays at line ends.
pub fn move_line_end(display: &DisplaySnapshot, selection: &mut Selection, n: isize) {
    let row = target_row(display, selection.cursor, n);
    to_line_end(selection, line_end(display, row));
}

/// Moves `selection` to `offset`, the end of a line, where vertical motion stays at
/// line ends. The mark stays.
fn to_line_end(selection: &mut Selection, offset: usize) {
    selection.cursor = offset;
    selection.goal = Some(Goal::LineEnd);
}

/// Moves `selection` as `C-a` does (mwim, ADR 0030): to the first non-blank
/// character of its line, or from there to the line's start. With `n` other than 1
/// it moves `n - 1` lines down first, and goes to the first non-blank character.
pub fn toggle_line_start(
    display: &DisplaySnapshot,
    selection: &mut Selection,
    n: isize,
    budget: &mut WalkBudget,
) {
    let row = target_row(display, selection.cursor, n);
    let code = first_non_blank(display, row, budget);
    selection.move_to(if n == 1 && selection.cursor == code {
        line_start(display, row)
    } else {
        code
    });
}

/// Moves `selection` as `C-e` does (mwim, ADR 0030): to the end of its line's code, before
/// trailing blanks and before a comment that `highlights` say ends the line (ADR 0111), or
/// from there to the line's end. With `n` other than 1 it moves `n - 1` lines down first, and
/// goes to the end of the code.
pub fn toggle_line_end(
    display: &DisplaySnapshot,
    selection: &mut Selection,
    (n, highlights): (isize, Option<&Highlights>),
    budget: &mut WalkBudget,
) {
    let row = target_row(display, selection.cursor, n);
    let code = code_end::code_end(display, row, highlights, budget);
    let end = line_end(display, row);
    if code == end || (n == 1 && selection.cursor == code) {
        to_line_end(selection, end);
    } else {
        selection.move_to(code);
    }
}

/// Whether `cluster` is blank: a tab, or text of nothing but white space, such as a
/// full-width space. Such text with a mark on it is not.
fn is_blank(cluster: &DisplayCluster<'_>) -> bool {
    match cluster.kind {
        CellKind::Tab => true,
        CellKind::Text => cluster.text.chars().all(char::is_whitespace),
        _ => false,
    }
}

/// Where the first non-blank cluster of `row` starts, or the row's end if it has
/// none. A run of blanks past what a stop reads ends the search where it got to.
fn first_non_blank(display: &DisplaySnapshot, row: usize, budget: &mut WalkBudget) -> usize {
    let mut walk = budget.stop();
    let mut end = line_start(display, row);
    if !walk.spend(SEEK_COST) {
        return end;
    }
    for cluster in display.clusters(DisplayPoint::new(row, 0)) {
        if !is_blank(&cluster) || !walk.spend(cluster.text.len()) {
            return cluster.offset;
        }
        end = cluster.offset + cluster.text.len();
    }
    end
}

/// Where the text of `row` up to `end` ends before the blanks it ends with, or the row's start
/// if it has nothing else. A run of blanks past what a stop reads ends the search where it got
/// to.
fn end_before_blanks(
    display: &DisplaySnapshot,
    (row, end): (usize, usize),
    budget: &mut WalkBudget,
) -> usize {
    let mut walk = budget.stop();
    let mut point = display.offset_to_display_point(end);
    let mut earliest = end;
    while point.column > 0 {
        let mut text_end = None;
        let mut first = None;
        let mut read = 0;
        for cluster in clusters_before(display, point, earliest) {
            first.get_or_insert((cluster.offset, cluster.column));
            read += cluster.text.len();
            if !is_blank(&cluster) {
                text_end = Some(cluster.offset + cluster.text.len());
            }
        }
        let spent = walk.spend(read + SEEK_COST);
        if let Some(text_end) = text_end {
            return text_end;
        }
        let Some((offset, column)) = first else {
            break;
        };
        earliest = offset;
        if !spent {
            return earliest;
        }
        point = DisplayPoint::new(row, column);
    }
    earliest
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

    #[test]
    fn cluster_end_finds_where_a_cursor_inside_one_goes() {
        // e + mark, tab, 日, CRLF, then a flag.
        let display = display("e\u{301}\t日\r\n🇯🇵");
        let expected = [
            (0, 0),
            (1, 3),
            (3, 3),
            (4, 4),
            (5, 7),
            (7, 7),
            (8, 9),
            (9, 9),
            (13, 17),
            (17, 17),
            (99, 17),
        ];
        for (offset, end) in expected {
            assert_eq!(cluster_end(&display, offset), end, "{offset}");
        }
    }

    #[test]
    fn cluster_start_finds_where_a_cursor_can_be() {
        // e + mark, tab, 日, CRLF, then a flag.
        let display = display("e\u{301}\t日\r\n🇯🇵");
        let expected = [
            (0, 0),
            (1, 0),
            (2, 0),
            (3, 3),
            (4, 4),
            (5, 4),
            (6, 4),
            (7, 7),
            (8, 7),
            (9, 9),
            (13, 9),
            (17, 17),
            (99, 17),
        ];
        for (offset, start) in expected {
            assert_eq!(cluster_start(&display, offset), start, "{offset}");
        }
    }

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
        let budget = &mut WalkBudget::new();
        assert_eq!(move_clusters(&d, &mut s, 2, budget), Ok(()));
        assert_eq!((s.cursor, s.goal), (4, None));
        assert_eq!(move_clusters(&d, &mut s, -1, budget), Ok(()));
        assert_eq!(s.cursor, 1);
        assert_eq!(move_clusters(&d, &mut s, 0, budget), Ok(()));
        assert_eq!(s.cursor, 1);
        // Goes as far as it can before reporting the edge.
        assert_eq!(move_clusters(&d, &mut s, 9, budget), Err(Edge::End));
        assert_eq!(s.cursor, 5);
        assert_eq!(move_clusters(&d, &mut s, -9, budget), Err(Edge::Start));
        assert_eq!(s.cursor, 0);
    }

    #[test]
    fn a_count_past_the_edge_by_clusters_but_not_by_bytes_reaches_it_in_a_walk() {
        // 3 clusters of 7 bytes; each way a count of 4 finds the edge by walking.
        let d = display("a日b\n語");
        let budget = &mut WalkBudget::new();
        let mut s = Selection::at(0);
        assert_eq!(move_clusters(&d, &mut s, 6, budget), Err(Edge::End));
        assert_eq!(s.cursor, 9);
        assert_eq!(move_clusters(&d, &mut s, -6, budget), Err(Edge::Start));
        assert_eq!(s.cursor, 0);
    }

    #[test]
    fn a_walk_stops_where_the_budget_runs_out() {
        // Each line break costs a seek.
        let d = display(&"\n".repeat(2 * MAX_COMMAND_WALK / SEEK_COST));
        let mut budget = WalkBudget::new();
        let mut s = Selection::at(0);
        let n = isize::try_from(d.row_count() - 1).unwrap();
        assert_eq!(move_clusters(&d, &mut s, n, &mut budget), Ok(()));
        assert!(budget.is_spent());
        assert_eq!(s.cursor, MAX_COMMAND_WALK / SEEK_COST - 1);
        let mut budget = WalkBudget::new();
        let mut s = Selection::at(d.row_count() - 1);
        assert_eq!(move_clusters(&d, &mut s, -n, &mut budget), Ok(()));
        assert!(budget.is_spent());
        assert_eq!(s.cursor, d.row_count() - MAX_COMMAND_WALK / SEEK_COST);
    }

    #[test]
    fn a_budget_spends_what_it_has_and_a_stop_less() {
        let mut budget = WalkBudget::new();
        assert!(budget.spend(MAX_COMMAND_WALK - 1));
        assert!(!budget.is_spent());
        assert!(budget.spend(1), "all it holds");
        assert!(!budget.is_spent(), "it went all the way");
        assert!(!budget.spend(1));
        assert!(budget.is_spent());
        let mut budget = WalkBudget::new();
        let mut stop = budget.stop();
        assert!(stop.spend(MAX_STOP_WALK));
        assert!(!stop.spend(1));
        assert!(!budget.is_spent(), "the command goes on");
        assert!(budget.stop().spend(1));
    }

    #[test]
    fn several_cursors_share_a_frames_reading_and_one_keeps_a_commands() {
        assert_eq!(WalkBudget::for_cursors(0), WalkBudget::new());
        assert_eq!(WalkBudget::for_cursors(1), WalkBudget::new());
        let mut budget = WalkBudget::for_cursors(2);
        assert_eq!(budget.left(), MAX_CURSORS_WALK);
        // A stop's worth each, for a few.
        for _ in 0..MAX_CURSORS_WALK / MAX_STOP_WALK {
            assert!(budget.stop().spend(MAX_STOP_WALK));
        }
        assert!(!budget.stop().spend(1));
        assert!(budget.is_spent());
    }

    #[test]
    fn several_cursors_get_entry_seeks_apart_from_the_bytes() {
        let mut budget = WalkBudget::for_cursors(2);
        for _ in 0..2 * ENTRY_SEEKS {
            assert!(budget.stop().enter());
        }
        assert_eq!(budget.left(), MAX_CURSORS_WALK, "no bytes taken");
        assert!(budget.stop().enter());
        assert_eq!(
            budget.left(),
            MAX_CURSORS_WALK - SEEK_COST,
            "past them, as bytes"
        );
        // One cursor, or a stop that has read nearly all it may, has none.
        let mut budget = WalkBudget::new();
        assert!(budget.stop().enter());
        assert_eq!(budget.left(), MAX_COMMAND_WALK - SEEK_COST);
        let mut budget = WalkBudget::for_cursors(2);
        let mut stop = budget.stop();
        assert!(stop.spend(MAX_STOP_WALK - 1));
        assert!(!stop.enter());
        // Nor does a command that has read all it may.
        let mut budget = WalkBudget::for_cursors(2);
        for _ in 0..MAX_CURSORS_WALK / MAX_STOP_WALK {
            assert!(budget.stop().spend(MAX_STOP_WALK));
        }
        assert!(!budget.stop().enter());
        assert!(budget.is_spent());
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
    fn the_cursor_box_goes_on_the_preedits_caret_or_after_it() {
        let d = display("a日b");
        let at = DisplayPoint::new(0, 1);
        assert_eq!(cursor_box(&d, at, None), (at, 2));
        let typing = Preedit::new("かな".into(), Some(6..6));
        assert_eq!(
            cursor_box(&d, at, Some(&typing)),
            (DisplayPoint::new(0, 5), 2),
            "after it, on 日"
        );
        let inside = Preedit::new("かな".into(), Some(3..3));
        assert_eq!(
            cursor_box(&d, at, Some(&inside)),
            (DisplayPoint::new(0, 3), 2)
        );
        let converting = Preedit::new("仮名".into(), Some(0..3));
        assert_eq!(
            cursor_box(&d, at, Some(&converting)),
            (DisplayPoint::new(0, 5), 2)
        );
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

    /// The rope tests' atoms, and characters that join others in ways of their own: Prepend
    /// marks, lone regional indicators, ZWJ and emoji, a virama, Hangul jamo, a keycap, a
    /// tag and a BOM.
    fn atom() -> impl Strategy<Value = &'static str> {
        const MORE: &[&str] = &[
            "\u{600}",
            "\u{d4e}",
            "\u{1f1ef}",
            "\u{1f1f5}",
            "\u{200d}",
            "\u{1f468}",
            "\u{915}\u{94d}",
            "\u{915}",
            "\u{94d}",
            "\u{301}",
            "\r",
            "\n",
            "a",
            "#",
            "\u{fe0f}",
            "\u{20e3}",
            "\u{1100}",
            "\u{1161}",
            "\u{11a8}",
            "\u{ac00}",
            "\u{e0001}",
            "\u{feff}",
        ];
        prop_oneof![
            prop::sample::select(crate::rope::tests::ATOMS),
            prop::sample::select(MORE),
        ]
    }

    proptest! {
        /// A cluster's start and end, found in one scan, are what a display point and
        /// back finds, and the next cluster after the start, at every offset of a text
        /// whose chunks edits have cut anywhere (ADR 0091).
        #[test]
        fn prop_a_cluster_is_found_as_through_a_display_point(
            atoms in prop::collection::vec(atom(), 0..60),
            edits in prop::collection::vec((0usize..400, prop::collection::vec(atom(), 0..4)), 0..6),
        ) {
            let mut rope = Rope::from(atoms.concat().as_str());
            for (at, insert) in edits {
                let at = crate::rope::tests::floor_boundary(&rope.to_string(), at);
                rope.insert(at, &insert.concat());
            }
            let map = DisplayMap::new(&rope, TabWidth::new(4));
            let d = map.snapshot();
            let text = rope.to_string();
            for offset in (0..=text.len() + 2).filter(|&offset| offset > text.len() || text.is_char_boundary(offset)) {
                let start = d.display_point_to_offset(d.offset_to_display_point(offset), Bias::Left);
                let end = if start >= offset.min(text.len()) {
                    start
                } else {
                    next_cluster(&d, start).unwrap_or(start)
                };
                prop_assert_eq!(cluster_start(&d, offset), start, "start at {}", offset);
                prop_assert_eq!(cluster_end(&d, offset), end, "end at {}", offset);
            }
        }

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

        /// A count ends where as many single moves do, each way.
        #[test]
        fn prop_a_count_is_as_many_moves(
            text in crate::rope::tests::text_strategy(40), start in 0usize..200, n in -30isize..30,
        ) {
            let d = display(&text);
            let all = stops(&d);
            let start = all[start % all.len()];
            let budget = &mut WalkBudget::new();
            let mut one_by_one = Selection::at(start);
            let mut edge = Ok(());
            for _ in 0..n.unsigned_abs() {
                edge = move_clusters(&d, &mut one_by_one, n.signum(), budget);
                if edge.is_err() {
                    break;
                }
            }
            let mut at_once = Selection::at(start);
            prop_assert_eq!(move_clusters(&d, &mut at_once, n, budget), edge);
            prop_assert_eq!(at_once.cursor, one_by_one.cursor);
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
