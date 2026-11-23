//! What a rewrap finds of the line it may stop in (`edit.rs`): whether a tab follows, whose cells
//! depend on its column, and whether the line's rows are indented as they were.
//!
//! Must not depend on: windows or the renderer.

use super::fit::Fit;
use super::{Bytes, Start, WrapItem, WrapSnapshot};
use crate::display_map::DisplaySnapshot;
use crate::sum_tree::Cursor;

impl WrapSnapshot {
    /// Whether a row from `index` of the item at `cursor`, which starts at `old`, to the end
    /// of its logical line holds a tab. What it found holds for later rows of one rewrap, kept
    /// in `known`, so that a long line is walked once.
    pub(super) fn tab_follows(
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
}

/// What a rewrap found of the line it may stop in, kept for its later rows.
#[derive(Default)]
pub(super) struct Ahead {
    pub(super) tabs: Option<(usize, TabAhead)>,
    /// The line, as `display` counts it, and whether its rows are indented as before.
    indent: Option<(usize, bool)>,
}

impl Ahead {
    /// Whether line `line` of `display` is indented as the one that held `old` in `before`.
    pub(super) fn same_indent(
        &mut self,
        fit: Fit,
        (before, display): (&DisplaySnapshot, &DisplaySnapshot),
        (old, line): (usize, usize),
    ) -> bool {
        if let Some((known, same)) = self.indent
            && known == line
        {
            return same;
        }
        let was = fit.indent(before, before.offset_to_display_point(old).row);
        let same = fit.indent(display, line) == was;
        self.indent = Some((line, same));
        same
    }
}

/// What follows a row on its line, as [`WrapSnapshot::tab_follows`] keeps it.
#[derive(Clone, Copy)]
pub(super) enum TabAhead {
    /// A row with a tab starts at this old offset.
    At(usize),
    /// No tab before the line ends at this old offset.
    None(usize),
}
