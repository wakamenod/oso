//! Markdown's indentation keys (ADR 0115): `TAB` cycling each cursor's line through the
//! columns it could take in its list, and `M-RET` starting a new list item after the current
//! one. Both read the items open at each cursor's line in one pass.
//!
//! Must not depend on: `command` or `keymap`; the commands call this.
//!
//! The cycle keeps no state between keys: a line at one of its stops goes to the next, and one
//! at none goes to the first, so that a keyboard macro replays the same.

use std::ops::Range;

use super::{IndentError, leading, spots};
use crate::buffer::{EditError, Mode, Replacement};
use crate::editor::Editor;
use crate::editor::tab::to_tab_stops;
use crate::indent::{self, Budget, Item, Lists, Marker};
use crate::rope::Rope;

/// The largest number a numbered item takes: CommonMark allows nine digits.
const MAX_NUMBER: u32 = 999_999_999;

/// What `TAB` does to one cursor's line in Markdown.
enum Cycle {
    /// Indent it to this column.
    To(usize),
    /// Spaces to the next tab stop at the cursor, inside a fenced code block.
    Stop,
    /// Leave it, past the budget.
    Keep,
}

impl Editor {
    /// `TAB` in Markdown: each cursor's line goes to the next of the columns it could take, the
    /// content columns of the items it could belong to from the innermost out, then 0, then the
    /// innermost again; a line at none of them goes to the first. Inside a fenced code block, a
    /// cursor gets spaces to the next tab stop instead. One edit.
    pub(super) fn markdown_tab(&mut self) -> Result<(), EditError> {
        let (window, buffer) = self.current();
        if let Some(reason) = buffer.read_only() {
            return Err(EditError::ReadOnly(reason));
        }
        let rope = buffer.text();
        let settings = buffer.indent_settings();
        let tab_width = settings.tab_width();
        let cursors: Vec<usize> = window.selections.iter().map(|s| s.cursor).collect();
        let mut rows: Vec<usize> = cursors
            .iter()
            .map(|&at| rope.offset_to_point(at).row)
            .collect();
        rows.dedup();
        let starts: Vec<usize> = rows.iter().map(|&row| rope.line_start(row)).collect();
        let cycles = cycles(rope, settings, &starts);
        let mut texts: Vec<(std::ops::Range<usize>, String)> = Vec::new();
        let mut cycled = Vec::new();
        let mut fenced = Vec::new();
        let mut index = 0;
        for (&row, (&start, cycle)) in rows.iter().zip(starts.iter().zip(&cycles)) {
            let on_row = cursors[index..]
                .iter()
                .take_while(|&&at| rope.offset_to_point(at).row == row)
                .count();
            match cycle {
                Cycle::Keep => {}
                Cycle::To(column) => {
                    cycled.push(row);
                    let (bytes, now) = leading(rope, start, tab_width);
                    if !super::has_indentation(rope, start..start + bytes, now, *column, settings) {
                        texts.push((start..start + bytes, settings.indentation(*column)));
                    }
                }
                Cycle::Stop => fenced.extend_from_slice(&cursors[index..index + on_row]),
            }
            index += on_row;
        }
        for (at, count) in to_tab_stops(buffer, fenced) {
            texts.push((at..at, " ".repeat(count)));
        }
        texts.sort_by_key(|(range, _)| range.start);
        let id = window.buffer();
        if !texts.is_empty() {
            let edits: Vec<_> = texts
                .iter()
                .map(|(range, text)| Replacement {
                    range: range.clone(),
                    text,
                })
                .collect();
            self.apply_edits(id, &edits)?;
        }
        self.cursors_past_indentation(&cycled);
        Ok(())
    }

    /// `M-RET` (`markdown-insert-list-item`): at each cursor, a new item of the innermost list
    /// item open there, after the line, with its indentation and marker, numbered one higher in
    /// a numbered list. The line breaks at the cursor, which goes to the new item's text; on a
    /// blank line the item goes on it, and with the cursor before an item's marker, on a new line
    /// before that item. One edit, inserting at most `limit` bytes.
    ///
    /// # Errors
    ///
    /// Nothing changes: a buffer not in Markdown, a read-only one, a cursor in no list item or in
    /// a fenced code block, or more than `limit` to insert.
    pub fn markdown_insert_list_item(
        &mut self,
        ending: &str,
        limit: usize,
    ) -> Result<(), IndentError> {
        let (window, buffer) = self.current();
        if buffer.mode() != Mode::Markdown || buffer.is_internal() {
            return Err(IndentError::NotInList);
        }
        if let Some(reason) = buffer.read_only() {
            return Err(EditError::ReadOnly(reason).into());
        }
        let rope = buffer.text();
        let settings = buffer.indent_settings();
        let cursors: Vec<usize> = window.selections.iter().map(|s| s.cursor).collect();
        let rows: Vec<usize> = cursors
            .iter()
            .map(|&at| rope.offset_to_point(at).row)
            .collect();
        // What is open once each cursor's line is read, the line itself too.
        let after: Vec<usize> = rows.iter().map(|&row| rope.line_start(row + 1)).collect();
        let lists = indent::markdown_lists(rope, settings, &after, &mut Budget::new());
        let spots = spots(rope, &cursors, Mode::Markdown);
        let mut planned = Vec::with_capacity(cursors.len());
        let mut inserted = 0usize;
        // The row whose whole line or indentation a cursor before replaced, and its cursor's
        // way back: one item a row, and the other cursors there go where its cursor goes.
        let mut whole: Option<(usize, usize)> = None;
        for (((&at, &row), lists), spot) in cursors.iter().zip(&rows).zip(lists).zip(&spots) {
            if let Some((_, back)) = whole.filter(|&(done, _)| done == row) {
                planned.push((at..at, String::new(), back));
                continue;
            }
            let Some(Lists {
                items,
                fenced: false,
                started,
            }) = lists
            else {
                return Err(IndentError::NotInList);
            };
            let Some(item) = items.last() else {
                return Err(IndentError::NotInList);
            };
            let start = rope.line_start(row);
            let (lead, _) = leading(rope, start, settings.tab_width());
            let blank = start + lead == rope.line_end(row);
            let before = started && !blank && at <= start + lead;
            let prefix = new_item_text(settings, item, !before);
            let (range, text, back) = if blank {
                whole = Some((row, 0));
                (start..rope.line_end(row), prefix, 0)
            } else if before {
                // The indentation goes back after the new line, and the cursor back over it, to
                // the new item's text.
                let kept = super::text_of(rope, start..start + lead);
                let back = ending.len() + kept.len();
                whole = Some((row, back));
                (start..start + lead, format!("{prefix}{ending}{kept}"), back)
            } else {
                (spot.range.clone(), format!("{ending}{prefix}"), 0)
            };
            inserted = inserted.saturating_add(text.len());
            if inserted > limit {
                return Err(IndentError::TooMuchText);
            }
            planned.push((range, text, back));
        }
        let id = window.buffer();
        let edits: Vec<_> = planned
            .iter()
            .map(|(range, text, _)| Replacement {
                range: range.clone(),
                text,
            })
            .collect();
        self.apply_edits(id, &edits)?;
        let targets = targets(&planned);
        let window = &mut self.windows[self.selected];
        if targets.len() == window.selections.len() {
            let mut targets = targets.into_iter();
            window.selections.update(|selection| {
                if let Some(target) = targets.next() {
                    selection.cursor = target;
                }
                Ok::<(), ()>(())
            });
        }
        Ok(())
    }
}

/// Where each cursor goes once `planned`, its range, text and way back, are in: to the end of
/// what went in at it, back over the line break and the indentation for a new item before its
/// line; an empty plan goes where the cursor before it went, as a cursor on one row with it. The
/// edit merged cursors that went to one place, and so does this.
fn targets(planned: &[(Range<usize>, String, usize)]) -> Vec<usize> {
    let mut targets: Vec<usize> = Vec::with_capacity(planned.len());
    let (mut added, mut removed) = (0usize, 0usize);
    for (range, text, back) in planned {
        let target = if range.is_empty() && text.is_empty() {
            targets
                .last()
                .copied()
                .unwrap_or(range.start + added - removed)
        } else {
            range.start + added - removed + text.len() - back
        };
        targets.push(target);
        added += text.len();
        removed += range.len();
    }
    targets.dedup();
    targets
}

/// What `TAB` does to each line that starts at one of `starts`, ascending, read in one pass.
fn cycles(rope: &Rope, settings: indent::Settings, starts: &[usize]) -> Vec<Cycle> {
    let lists = indent::markdown_lists(rope, settings, starts, &mut Budget::new());
    starts
        .iter()
        .zip(lists)
        .map(|(&start, lists)| match lists {
            None => Cycle::Keep,
            Some(lists) if lists.fenced => Cycle::Stop,
            Some(lists) => {
                let column = leading(rope, start, settings.tab_width()).1;
                Cycle::To(next_stop(&lists, column))
            }
        })
        .collect()
}

/// The column after `column` among the stops of `lists`; past the last, the first, and from a
/// column at none of them, the first.
fn next_stop(lists: &Lists, column: usize) -> usize {
    let stops = lists.stops();
    match stops.iter().position(|&stop| stop == column) {
        Some(at) => stops[(at + 1) % stops.len()],
        None => stops[0],
    }
}

/// What a new item like `item` starts with: its indentation, its marker, numbered one higher
/// if `next`, and the spaces to its text, fewer by as much as the number grew, but one at least.
fn new_item_text(settings: indent::Settings, item: &Item, next: bool) -> String {
    let marker = match item.marker {
        Marker::Bullet(bullet) => char::from(bullet).to_string(),
        Marker::Number { number, delimiter } => {
            let number = if next && number < MAX_NUMBER {
                number + 1
            } else {
                number
            };
            format!("{number}{}", char::from(delimiter))
        }
    };
    let was = match item.marker {
        Marker::Bullet(_) => 1,
        Marker::Number { number, .. } => digits(number) + 1,
    };
    let gap = item.content.saturating_sub(item.indent).saturating_sub(was);
    let gap = gap.saturating_sub(marker.len().saturating_sub(was)).max(1);
    format!(
        "{}{marker}{}",
        settings.indentation(item.indent),
        " ".repeat(gap)
    )
}

/// How many digits `number` is written with.
fn digits(number: u32) -> usize {
    number.checked_ilog10().map_or(1, |log| log as usize + 1)
}

#[cfg(test)]
mod tests;
