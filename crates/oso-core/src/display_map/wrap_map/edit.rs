//! How a [`WrapSnapshot`] follows an edit (ADR 0100): it rewraps from the row before the
//! edit, until a row starts where one did before, past the edit and at the same column
//! modulo the tab width, and splices those rows in place of the old ones.
//!
//! Must not depend on: windows or the renderer; it reads the display snapshot after the edit
//! and the rows from before it.

use super::breaks;
use super::{Builder, Bytes, Row, RowStream, Start, WrapItem, WrapSnapshot};
use crate::display_map::{DisplayPoint, DisplaySnapshot, MAX_CLUSTER_BYTES};
use crate::rope::RopeEdit;
use crate::sum_tree::{Bias, Cursor};

use std::ops::Range;

impl WrapSnapshot {
    /// The rows after `edit`, which made `display` from `before`, the snapshot these rows
    /// were made or last edited with. Rewraps from the row before the edit until a row starts
    /// where one did before, past the edit, at the same column modulo the tab width if a tab
    /// follows on its line; every row after that is as it was.
    pub fn edited(
        &self,
        before: &DisplaySnapshot,
        display: &DisplaySnapshot,
        edit: &RopeEdit,
    ) -> Self {
        let mut unlimited = usize::MAX;
        self.edited_within(before, display, edit, &mut unlimited).0
    }

    /// [`WrapSnapshot::edited`] reading at most `budget` bytes of rows, which it takes from.
    /// Past the budget it keeps the old rows, joined to the new ones by provisional rows, and
    /// returns the stale range, from where it stopped to the old row it joined, for a
    /// [`WrapJob`](super::WrapJob) to rewrap on until its rows meet the old ones (ADR 0100,
    /// ADR 0102). A window takes a command's budget
    /// from [`COMMAND_BUDGET`](super::COMMAND_BUDGET).
    pub fn edited_within(
        &self,
        before: &DisplaySnapshot,
        display: &DisplaySnapshot,
        edit: &RopeEdit,
        budget: &mut usize,
    ) -> (Self, Option<Range<usize>>) {
        let never = || false;
        self.edited_unless((before, display), edit, budget, (&never, true))
            .map_or_else(
                || (self.clone(), Some(edit.new.clone())),
                |(wrap, stale, _)| (wrap, stale),
            )
    }

    /// [`WrapSnapshot::edited_within`], or `None` once `stop` says so, which it asks every
    /// [`STOP_ROWS`](super::STOP_ROWS) rows. Unless `rows_from_before`, the old rows may be
    /// provisional ones, kept from before an edit, whose tabs had other columns, so that no
    /// rewrap stops where a tab follows on the line.
    fn edited_unless(
        &self,
        (before, display): (&DisplaySnapshot, &DisplaySnapshot),
        edit: &RopeEdit,
        budget: &mut usize,
        (stop, rows_from_before): (&dyn Fn() -> bool, bool),
    ) -> Option<(Self, Option<Range<usize>>, usize)> {
        let old_end = self.tree.summary().bytes;
        if old_end == 0 {
            // An empty buffer's one row has no bytes to place a splice by.
            return Some((Self::with_width(display, self.width), None, display.len()));
        }
        let delta = edit.new.len() as isize - edit.old.len() as isize;
        let from = self.rewrap_start(display, edit.old.start.min(old_end));
        let mut fresh = Vec::new();
        // Where the old rows take over, in the old offsets, or the end.
        let mut until = old_end;
        let mut stale = None;
        if *budget == 0 {
            let (wrap, stale) = self.spent((before, display), edit.old.clone(), (delta, 0));
            return Some((wrap, stale, edit.new.end));
        } else {
            *budget = budget.saturating_sub(super::job::SPLICE_COST);
            let mut stream = RowStream::new(display, from, self.width);
            let mut tabs = None;
            while let Some(row) = stream.next() {
                if fresh.len() % super::STOP_ROWS == super::STOP_ROWS - 1 && stop() {
                    return None;
                }
                fresh.push(Fresh::Row(row));
                *budget = budget.saturating_sub(row.len as usize);
                let (at, column) = stream.position();
                if at > edit.new.end {
                    let old = at.checked_add_signed(-delta).unwrap_or(0);
                    if self.starts_row_at(
                        (before, display),
                        (old, at),
                        (column, rows_from_before),
                        &mut tabs,
                    ) {
                        until = old;
                        break;
                    }
                }
                if *budget == 0 && at < display.len() {
                    let near = if at > edit.new.end {
                        at.checked_add_signed(-delta).unwrap_or(0)
                    } else {
                        edit.old.end
                    };
                    let (join, range) =
                        self.join_old((before, display), (at, near), delta, &mut fresh);
                    (until, stale) = (join, Some(range));
                    break;
                }
            }
        }
        let mut tree = self.tree.clone();
        let (first, last, items) = self.splice(before, (from, until), &fresh);
        tree.replace_items(Bytes(first)..Bytes(last), items);
        let wrap = Self {
            tree,
            width: self.width,
        };
        Some((wrap, stale, until.checked_add_signed(delta).unwrap_or(0)))
    }

    /// The rows after edits of `old`, rows that `before` showed, which changed its length by
    /// `delta` and made `display`, with a command's budget spent: the old rows around them
    /// stay, and the rows from the one that held their start to the first old boundary past
    /// their end are provisional, read by no rule. Returns them and the stale range, from the
    /// row before, which kinsoku may change too, in `display`'s offsets.
    ///
    /// What precedes `old` in these rows reads as in `before`, and in `display` it lies `base`
    /// further on, as other edits there, not yet followed, may have moved it.
    pub(super) fn spent(
        &self,
        (before, display): (&DisplaySnapshot, &DisplaySnapshot),
        old: Range<usize>,
        (delta, base): (isize, isize),
    ) -> (Self, Option<Range<usize>>) {
        let old_end = self.tree.summary().bytes;
        if old_end == 0 || display.len() == 0 {
            // An empty buffer's one row has no bytes to place a splice by.
            return (Self::with_width(display, self.width), None);
        }
        let row = self.row_of(before, old.start.min(old_end));
        let from = self.row_span(before, row).0;
        let stale_from = match row.checked_sub(1).map(|row| self.row_span(before, row)) {
            Some((start, _, true)) => start,
            _ => from,
        };
        let join = self.old_boundary(before, old.end.min(old_end));
        let now = |at: usize| at.checked_add_signed(base).unwrap_or(0);
        let join_now = now(join.checked_add_signed(delta).unwrap_or(0)).max(now(from));
        let mut fresh = Vec::new();
        provisional(display, (now(from), join_now), &mut fresh);
        let mut tree = self.tree.clone();
        let (first, last, items) = self.splice(before, (from, join), &fresh);
        tree.replace_items(Bytes(first)..Bytes(last), items);
        let wrap = Self {
            tree,
            width: self.width,
        };
        (wrap, Some(now(stale_from)..join_now))
    }

    /// Joins the new rows, which reach `at`, to the old ones at the first old row boundary at
    /// or after `near`, with provisional rows between. Returns that boundary, in the old
    /// offsets, and the stale range a job should rewrap from: to the boundary, past which the
    /// job goes on until its rows meet the old ones.
    pub(super) fn join_old(
        &self,
        (before, display): (&DisplaySnapshot, &DisplaySnapshot),
        (at, near): (usize, usize),
        delta: isize,
        fresh: &mut Vec<Fresh>,
    ) -> (usize, Range<usize>) {
        let old_end = self.tree.summary().bytes;
        let join = self.old_boundary(before, near.min(old_end));
        let join_new = join.checked_add_signed(delta).unwrap_or(0).max(at);
        provisional(display, (at, join_new), fresh);
        (join, at..join_new)
    }

    /// The rows with those of `range` in `display` wrapped again, as a stale range is, and
    /// where the rewrap reached, past which the rows are as before; or `None` once `stop` says
    /// so.
    pub(super) fn rewrapped_unless(
        &self,
        display: &DisplaySnapshot,
        range: Range<usize>,
        stop: &dyn Fn() -> bool,
    ) -> Option<(Self, usize)> {
        let edit = RopeEdit {
            old: range.clone(),
            new: range,
        };
        let mut unlimited = usize::MAX;
        let (wrap, _, reached) =
            self.edited_unless((display, display), &edit, &mut unlimited, (stop, false))?;
        Some((wrap, reached))
    }

    /// The first place at or after `near` where a row started in `before`, which these rows
    /// were made with. Reads the tree alone in an item of rows.
    pub(super) fn old_boundary(&self, before: &DisplaySnapshot, near: usize) -> usize {
        let mut cursor = self.tree.cursor::<(Bytes, Start)>();
        cursor.seek(&Bytes(near), Bias::Right);
        let start = cursor.start().1;
        match cursor.item() {
            None => self.tree.summary().bytes,
            Some(WrapItem::Lines { .. }) => {
                // Only where its line starts: a line break of two bytes is one cluster.
                let point = before.offset_to_display_point(near);
                let line_start = before.display_point_to_offset(point, Bias::Left);
                if point.column == 0 && line_start == near {
                    near
                } else {
                    super::line_end(before, point.row, before.len())
                }
            }
            Some(WrapItem::Rows(rows)) => {
                let mut at = start.bytes;
                for row in rows.iter() {
                    if at >= near {
                        return at;
                    }
                    at += row.len as usize;
                }
                at
            }
        }
    }

    /// Where a rewrap for an edit at `at` starts: the row before the one that holds `at`,
    /// on the same logical line, and further back while those rows hold too few clusters for
    /// kinsoku's reach, or while a row before reaches `at`. `at` and what precedes it read the
    /// same in `display` as before.
    ///
    /// That holds because `at` starts the chunks the rope rebuilt, which takes the chunk that
    /// ends at the edit too, and no display cluster crosses into them from before: an
    /// invisible or an escape is a cluster of its own. A row before `at` can then change only
    /// at its end, where kinsoku and a hanging space read up to [`breaks::KINSOKU_REACH`]
    /// clusters into the next row, which the rows stepped back over cover. A row that broke at
    /// the last break before a stretch with none, as `aa` before `あ。。  。`, reads on to the
    /// cluster that does not fit and one more: a break the edit makes there moves its end
    /// (ADR 0125).
    pub(super) fn rewrap_start(&self, display: &DisplaySnapshot, at: usize) -> usize {
        let mut row = self.row_of(display, at);
        let mut start = self.row_span(display, row).0;
        // A row reads the cluster at `at` when the one before it starts within the row's
        // width: no cluster before that one overflows it. Left of a zero-cell cluster this
        // finds an earlier one, which steps back over more rows, never fewer.
        let point = display.offset_to_display_point(at);
        let before_at = point
            .column
            .checked_sub(1)
            .and_then(|column| {
                display
                    .clusters(DisplayPoint::new(point.row, column))
                    .next()
            })
            .map(|cluster| cluster.column);
        let reads_at = |row_start: usize| {
            before_at.is_some_and(|column| {
                column.saturating_sub(display.offset_to_display_point(row_start).column)
                    <= self.width
            })
        };
        let (mut steps, mut short) = (0, true);
        while row > 0 {
            let (before, end, continues) = self.row_span(display, row - 1);
            if !continues || !((short && steps <= breaks::KINSOKU_REACH) || reads_at(before)) {
                break;
            }
            row -= 1;
            start = before;
            steps += 1;
            // A row longer than two clusters can be holds more than two.
            short = end - before <= 2 * MAX_CLUSTER_BYTES
                && display
                    .clusters(display.offset_to_display_point(before))
                    .take_while(|cluster| cluster.offset < end)
                    .nth(breaks::KINSOKU_REACH)
                    .is_none();
        }
        start
    }

    /// Whether a row started at `old` in `before`, as one now starts at `at`, the same place
    /// in `display` after the edit with what precedes it unchanged, at `column`. Past a tab on
    /// the line, the columns must also agree modulo the tab width, for its cells to.
    fn starts_row_at(
        &self,
        (before, display): (&DisplaySnapshot, &DisplaySnapshot),
        (old, at): (usize, usize),
        (column, rows_from_before): (usize, bool),
        tabs: &mut Option<(usize, TabAhead)>,
    ) -> bool {
        let mut cursor = self.tree.cursor::<(Bytes, Start)>();
        cursor.seek(&Bytes(old), Bias::Right);
        let start = cursor.start().1;
        match cursor.item() {
            None => false,
            Some(WrapItem::Lines { .. }) => {
                old == start.bytes || display.offset_to_display_point(at).column == 0
            }
            Some(WrapItem::Rows(rows)) => {
                let mut row_start = start.bytes;
                for index in 0..rows.len() {
                    if row_start == old {
                        if !Self::tab_follows(&mut cursor, (index, old), tabs) {
                            return true;
                        }
                        if !rows_from_before {
                            return false;
                        }
                        let tab_width = display.tab_width().get() as usize;
                        let was = before.offset_to_display_point(old).column;
                        return was % tab_width == column % tab_width;
                    }
                    row_start += rows[index].len as usize;
                }
                false
            }
        }
    }

    /// Whether a row from `index` of the item at `cursor`, which starts at `old`, to the end
    /// of its logical line holds a tab. What it found holds for later rows of one rewrap, kept
    /// in `known`, so that a long line is walked once.
    fn tab_follows(
        cursor: &mut Cursor<'_, WrapItem, (Bytes, Start)>,
        (index, old): (usize, usize),
        known: &mut Option<(usize, TabAhead)>,
    ) -> bool {
        match *known {
            Some((from, TabAhead::At(tab))) if from <= old && old <= tab => return true,
            Some((from, TabAhead::None(until))) if from <= old && old < until => return false,
            _ => {}
        }
        let ahead = Self::tab_ahead(cursor, (index, old));
        *known = Some((old, ahead));
        matches!(ahead, TabAhead::At(_))
    }

    /// Where the first row from `index` of the item at `cursor`, which starts at `old`, holds a
    /// tab on the same line, or where that line ends with none.
    fn tab_ahead(
        cursor: &mut Cursor<'_, WrapItem, (Bytes, Start)>,
        (index, old): (usize, usize),
    ) -> TabAhead {
        let mut first = Some(index);
        let mut at = old;
        while let Some(item) = cursor.item() {
            let WrapItem::Rows(rows) = item else {
                return TabAhead::None(at);
            };
            let skip = first.take().unwrap_or(0);
            let whole = skip == 0 && rows.iter().all(|row| row.continues);
            if whole && !cursor.item_summary().is_some_and(|summary| summary.tabs) {
                at += cursor.item_summary().map_or(0, |summary| summary.bytes);
            } else {
                for row in &rows[skip..] {
                    if row.tabs {
                        return TabAhead::At(at);
                    }
                    at += row.len as usize;
                    if !row.continues {
                        return TabAhead::None(at);
                    }
                }
            }
            cursor.next();
        }
        TabAhead::None(at)
    }

    /// The items that replace the old rows from `from` to `until`, both old offsets where a
    /// row starts, with `fresh`: the old items that hold them, cut where `from` and `until`
    /// fall, around the new rows. Returns the old items' range with them. It reads `before`,
    /// which these rows were made with, at no offset past `until`, so that rows further on may
    /// already follow other edits.
    pub(super) fn splice(
        &self,
        before: &DisplaySnapshot,
        (from, until): (usize, usize),
        fresh: &[Fresh],
    ) -> (usize, usize, Vec<WrapItem>) {
        let mut cursor = self.tree.cursor::<(Bytes, Start)>();
        cursor.seek(&Bytes(from), Bias::Right);
        if cursor.item().is_none() {
            cursor.seek(&Bytes(from), Bias::Left);
        }
        let head = cursor.start().1;
        // Whether a line starts at the item, as the row before it says.
        let starts_line = head
            .rows
            .checked_sub(1)
            .is_none_or(|row| !self.row_span(before, row).2);
        let mut builder = Builder::new(starts_line);
        match cursor.item() {
            Some(WrapItem::Lines { .. }) if from > head.bytes => {
                let lines = before.offset_to_display_point(from).row - head.lines;
                builder.lines(u32::try_from(lines).unwrap_or(u32::MAX), from - head.bytes);
            }
            Some(WrapItem::Rows(rows)) => {
                let mut at = head.bytes;
                for row in rows.iter().take_while(|row| {
                    let before = at < from;
                    at += row.len as usize;
                    before
                }) {
                    builder.row(*row);
                }
            }
            _ => {}
        }
        for piece in fresh {
            match *piece {
                Fresh::Row(row) => builder.row(row),
                Fresh::Lines(lines, bytes) => builder.lines(lines, bytes),
            }
        }
        let old_end = self.tree.summary().bytes;
        let mut last = old_end;
        if until < old_end {
            let mut cursor = self.tree.cursor::<(Bytes, Start)>();
            cursor.seek(&Bytes(until), Bias::Right);
            let tail = cursor.start().1;
            let item_end = tail.bytes + cursor.item_summary().map_or(0, |summary| summary.bytes);
            last = item_end;
            match cursor.item() {
                Some(WrapItem::Lines { lines, .. }) if until > tail.bytes => {
                    // The item's lines past `until`: its own count, less those before.
                    let passed = before.offset_to_display_point(until).row - tail.lines;
                    let remaining = (*lines as usize).saturating_sub(passed);
                    builder.lines(
                        u32::try_from(remaining).unwrap_or(u32::MAX),
                        item_end - until,
                    );
                }
                Some(WrapItem::Rows(rows)) => {
                    let mut at = tail.bytes;
                    for row in rows.iter() {
                        if at >= until {
                            builder.row(*row);
                        }
                        at += row.len as usize;
                    }
                }
                Some(WrapItem::Lines { .. }) | None => last = until,
            }
            if last == old_end {
                // The empty line after a last line break has no bytes, so the range takes
                // it too, and it goes back.
                cursor.next();
                while let Some(WrapItem::Lines { lines, bytes: 0 }) = cursor.item() {
                    builder.lines(*lines, 0);
                    cursor.next();
                }
            }
        }
        (head.bytes, last, builder.finish())
    }
}

/// What a rewrap puts in place of old rows: rows, or provisional lines of one row each.
#[derive(Clone, Copy)]
pub(super) enum Fresh {
    Row(Row),
    Lines(u32, usize),
}

/// The most bytes [`provisional`] searches for line breaks; past it, it finds the lines from
/// the display map, in O(log n), so that a long provisional row costs no more than a short one.
const SEARCHED: usize = 4 * 1024;

/// Provisional rows over `range` of `display`, a row's start to an old row's: each logical
/// line one row, unwrapped, and the empty line after a last line break if the range reaches
/// it. A short range with no line break but at its end, as a spent splice's within a long
/// line, takes one search of its bytes and no scan.
pub(super) fn provisional(
    display: &DisplaySnapshot,
    range: (usize, usize),
    fresh: &mut Vec<Fresh>,
) {
    let (at, end) = range;
    let row = |len: usize, continues: bool| {
        Fresh::Row(Row {
            len: u32::try_from(len).unwrap_or(u32::MAX),
            continues,
            // Unknown, so a later resync here compares columns.
            tabs: true,
        })
    };
    if at < end {
        let single = end - at <= SEARCHED && {
            let mut breaks = 0;
            let mut last = 0;
            let mut offset = at;
            for piece in display.text_in(at..end) {
                for (index, _) in piece.match_indices('\n') {
                    breaks += 1;
                    last = offset + index;
                }
                offset += piece.len();
            }
            breaks == 0 || (breaks == 1 && last + 1 == end)
        };
        if single {
            let ends_line =
                end == display.len() || display.offset_to_display_point(end).column == 0;
            fresh.push(row(end - at, !ends_line));
        } else {
            lines_between(display, (at, end), fresh, &row);
        }
    }
    let point = display.max_point();
    if end == display.len() && point.row > 0 && point.column == 0 {
        // The empty line after a last line break, which no byte holds.
        fresh.push(row(0, false));
    }
}

/// [`provisional`]'s rows for a long range: its first line's part, its whole lines, and its
/// last line's part, each found from the display map.
fn lines_between(
    display: &DisplaySnapshot,
    (at, end): (usize, usize),
    fresh: &mut Vec<Fresh>,
    row: &dyn Fn(usize, bool) -> Fresh,
) {
    let len = display.len();
    let first = display.offset_to_display_point(at).row;
    let first_end = super::line_end(display, first, len);
    if end <= first_end {
        fresh.push(row(end - at, end < first_end));
        return;
    }
    fresh.push(row(first_end - at, false));
    let last = display.offset_to_display_point(end).row;
    let last_start = display.display_point_to_offset(DisplayPoint::new(last, 0), Bias::Left);
    let whole = last - (first + 1);
    if whole > 0 {
        fresh.push(Fresh::Lines(
            u32::try_from(whole).unwrap_or(u32::MAX),
            last_start - first_end,
        ));
    }
    if end > last_start {
        fresh.push(row(
            end - last_start,
            end < super::line_end(display, last, len),
        ));
    }
}

/// What follows a row on its line, as [`WrapSnapshot::tab_follows`] keeps it.
#[derive(Clone, Copy)]
enum TabAhead {
    /// A row with a tab starts at this old offset.
    At(usize),
    /// No tab before the line ends at this old offset.
    None(usize),
}
