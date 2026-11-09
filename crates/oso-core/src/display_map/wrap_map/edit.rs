//! How a [`WrapSnapshot`] follows an edit (ADR 0100): it rewraps from the row before the
//! edit, until a row starts where one did before, past the edit and at the same column
//! modulo the tab width, and splices those rows in place of the old ones.
//!
//! Must not depend on: windows or the renderer; it reads the display snapshot after the edit
//! and the rows from before it.

use super::breaks;
use super::{Builder, Bytes, Row, RowStream, Start, WrapItem, WrapSnapshot};
use crate::display_map::{DisplaySnapshot, MAX_CLUSTER_BYTES};
use crate::rope::RopeEdit;
use crate::sum_tree::{Bias, Cursor};

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
        let old_end = self.tree.summary().bytes;
        if old_end == 0 {
            // An empty buffer's one row has no bytes to place a splice by.
            return Self::with_width(display, self.width);
        }
        let delta = edit.new.len() as isize - edit.old.len() as isize;
        let from = self.rewrap_start(display, edit.old.start.min(old_end));
        let mut stream = RowStream::new(display, from, self.width);
        let mut fresh = Vec::new();
        // Where the old rows take over, in the old offsets, or the end.
        let mut until = old_end;
        while let Some(row) = stream.next() {
            fresh.push(row);
            let (at, column) = stream.position();
            if at > edit.new.end {
                let old = at.checked_add_signed(-delta).unwrap_or(0);
                if self.starts_row_at((before, display), (old, at), column) {
                    until = old;
                    break;
                }
            }
        }
        let mut tree = self.tree.clone();
        let (first, last, items) = self.splice(display, (from, until), (delta, &fresh));
        tree.replace_items(Bytes(first)..Bytes(last), items);
        Self {
            tree,
            width: self.width,
        }
    }

    /// Where a rewrap for an edit at `at` starts: the row before the one that holds `at`,
    /// on the same logical line, and further back while those rows hold too few clusters for
    /// kinsoku's reach. `at` and what precedes it read the same in `display` as before.
    fn rewrap_start(&self, display: &DisplaySnapshot, at: usize) -> usize {
        let mut row = self.row_of(display, at);
        let mut start = self.row_span(display, row).0;
        for _ in 0..=breaks::KINSOKU_REACH {
            if row == 0 {
                break;
            }
            let (before, end, continues) = self.row_span(display, row - 1);
            if !continues {
                break;
            }
            row -= 1;
            start = before;
            // A row longer than two clusters can be holds more than two.
            let short = end - before <= 2 * MAX_CLUSTER_BYTES
                && display
                    .clusters(display.offset_to_display_point(before))
                    .take_while(|cluster| cluster.offset < end)
                    .nth(breaks::KINSOKU_REACH)
                    .is_none();
            if !short {
                break;
            }
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
        column: usize,
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
                        if !self.tab_follows(&mut cursor, index) {
                            return true;
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

    /// Whether a row from `index` of the item at `cursor` to the end of its logical line holds
    /// a tab. Reads whole items past the first by their summaries.
    fn tab_follows(&self, cursor: &mut Cursor<'_, WrapItem, (Bytes, Start)>, index: usize) -> bool {
        let mut first = Some(index);
        while let Some(item) = cursor.item() {
            let WrapItem::Rows(rows) = item else {
                return false;
            };
            let skip = first.take().unwrap_or(0);
            let ends_line = rows[skip..].iter().any(|row| !row.continues);
            if !ends_line && skip == 0 {
                if cursor.item_summary().is_some_and(|summary| summary.tabs) {
                    return true;
                }
            } else {
                for row in &rows[skip..] {
                    if row.tabs {
                        return true;
                    }
                    if !row.continues {
                        return false;
                    }
                }
            }
            cursor.next();
        }
        false
    }

    /// The items that replace the old rows from `from` to `until`, both old offsets where a
    /// row starts, with `fresh`: the old items that hold them, cut where `from` and `until`
    /// fall, around the new rows. Returns the old items' range with them.
    fn splice(
        &self,
        display: &DisplaySnapshot,
        (from, until): (usize, usize),
        (delta, fresh): (isize, &[Row]),
    ) -> (usize, usize, Vec<WrapItem>) {
        let mut cursor = self.tree.cursor::<(Bytes, Start)>();
        cursor.seek(&Bytes(from), Bias::Right);
        if cursor.item().is_none() {
            cursor.seek(&Bytes(from), Bias::Left);
        }
        let head = cursor.start().1;
        let starts_line = display.offset_to_display_point(head.bytes).column == 0;
        let mut builder = Builder::new(starts_line);
        match cursor.item() {
            Some(WrapItem::Lines { .. }) if from > head.bytes => {
                let lines = display.offset_to_display_point(from).row - head.lines;
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
        for row in fresh {
            builder.row(*row);
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
                Some(WrapItem::Lines { .. }) if until > tail.bytes => {
                    let at = until.checked_add_signed(delta).unwrap_or(0);
                    let new_end = item_end.checked_add_signed(delta).unwrap_or(0);
                    let line = display.offset_to_display_point(at).row;
                    let remaining = if item_end == old_end {
                        display.row_count() - line - self.empty_lines_after(item_end)
                    } else {
                        display.offset_to_display_point(new_end).row - line
                    };
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

    /// The lines in items of no bytes after the item that ends at `end`: the empty line
    /// after a last line break, when it is an item of its own.
    fn empty_lines_after(&self, end: usize) -> usize {
        let mut cursor = self.tree.cursor::<Bytes>();
        cursor.seek(&Bytes(end), Bias::Left);
        cursor.next();
        let mut lines = 0;
        while let Some(WrapItem::Lines {
            lines: more,
            bytes: 0,
        }) = cursor.item()
        {
            lines += *more as usize;
            cursor.next();
        }
        lines
    }
}
