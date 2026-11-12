//! A line's indentation at every cursor (ADR 0115): `back-to-indentation` (`M-m`), which moves
//! past it, and `delete-indentation` (`M-^`), which joins a line to the one before over it,
//! leaving one space or none, as Emacs's `fixup-whitespace` does.
//!
//! Must not depend on: `command` or `keymap`; the commands call this.
//!
//! Indentation is spaces and tabs only, as `indent` counts it: a full-width space is text. What
//! is read at a line's edge is at most [`SCAN`](super::SCAN) bytes, so a line of a gigabyte of spaces costs no
//! more than a short one.

use std::ops::Range;

use super::leads::{leads_of, read_cost};
use super::{IndentError, blanks_before, leading};
use crate::buffer::{EditError, Replacement};
use crate::display_map::TabWidth;
use crate::editor::Editor;
use crate::rope::Rope;
use crate::selection::SelectionSet;

/// The most lines one `M-^` joins: a join removes a line break, a splice of about 2 µs, so that
/// the command stays under 50 ms with its reading (ADR 0115, step 3).
pub(crate) const MAX_JOINS: usize = 15_000;

/// What one `M-^` over a region may read, as [`read_cost`] counts it.
pub(crate) const JOIN_READ: usize = 2 << 20;

impl Editor {
    /// `M-m` (`back-to-indentation`): moves each cursor of the selected window to the first
    /// character of its line past spaces and tabs, at most [`SCAN`](super::SCAN) bytes in. Returns whether a
    /// cursor moved.
    pub fn back_to_indentation(&mut self) -> bool {
        let (window, buffer) = self.current_mut();
        let rope = buffer.text();
        let tab_width = buffer.tab_width();
        let display = buffer.display();
        window
            .selections
            .update(|selection| {
                let start = rope.line_start(rope.offset_to_point(selection.cursor).row);
                let end = start + leading(rope, start, tab_width).0;
                selection.move_to(crate::motion::cluster_start(&display, end));
                Ok::<(), ()>(())
            })
            .changed
    }

    /// `M-^` (`delete-indentation`): joins each cursor's line to the one before, or with `next`
    /// the line after to it, or with a region shown and not `next`, every line of each region to
    /// the one before. The blanks around each join go, and one space goes in, but none at a
    /// line's start or end, after an opening bracket or before a closing one. Each cursor goes to
    /// where its region's, or its line's, first join is. One edit.
    ///
    /// # Errors
    ///
    /// Nothing changes: a read-only buffer, or more lines to join than one command takes.
    pub fn delete_indentation(&mut self, next: bool) -> Result<(), IndentError> {
        let (window, buffer) = self.current();
        if let Some(reason) = buffer.read_only() {
            return Err(EditError::ReadOnly(reason).into());
        }
        let rope = buffer.text();
        let set = &window.selections;
        let spans = join_spans(rope, set, next);
        let runs = runs_of(&spans);
        let joins: usize = runs.iter().map(ExactSizeIterator::len).sum();
        // A single join reads only the edges of its lines; a run of them, every line once.
        let read: Vec<Range<usize>> = runs
            .iter()
            .filter(|run| run.len() > 1)
            .map(|run| run.start - 1..run.end)
            .collect();
        if joins > MAX_JOINS || read_cost(rope, &read) > JOIN_READ {
            return Err(IndentError::TooLarge);
        }
        let joined = joined(rope, buffer.tab_width(), &runs);
        if joined.is_empty() {
            return Ok(());
        }
        // Where each join starts once those before it are made.
        let mut shifted = Vec::with_capacity(joined.len());
        let (mut added, mut removed) = (0, 0);
        for join in &joined {
            shifted.push((join.range.start + added).saturating_sub(removed));
            added += join.space.len();
            removed += join.range.len();
        }
        // Each cursor goes to the start of its first join.
        let targets: Vec<Option<usize>> = spans
            .iter()
            .map(|span| {
                let first = Some(span.start).filter(|_| !span.is_empty())?;
                let at = joined.partition_point(|join| join.rows.end <= first);
                joined.get(at).filter(|join| join.rows.contains(&first))?;
                Some(shifted[at])
            })
            .collect();
        let id = window.buffer();
        let edits: Vec<_> = joined
            .iter()
            .map(|join| Replacement {
                range: join.range.clone(),
                text: join.space,
            })
            .collect();
        self.apply_edits(id, &edits)?;
        let window = &mut self.windows[self.selected];
        if targets.len() == window.selections.len() {
            let mut targets = targets.into_iter();
            window.selections.update(|selection| {
                if let Some(Some(at)) = targets.next() {
                    selection.move_to(at);
                }
                Ok::<(), ()>(())
            });
        }
        Ok(())
    }
}

/// The rows each selection of `set` joins to the row before: its own, with `next` the one
/// after it, or with a region shown and not `next`, the region's rows past its first. Empty for
/// none, as at the buffer's first line.
fn join_spans(rope: &Rope, set: &SelectionSet, next: bool) -> Vec<Range<usize>> {
    let regions = !next && set.mark_active().is_active();
    let last = rope.offset_to_point(rope.len()).row;
    set.iter()
        .map(|selection| {
            let row = rope.offset_to_point(selection.cursor).row;
            // An empty region is no region, as Emacs's `use-region-p` says.
            if regions && !selection.range().is_empty() {
                let range = selection.range();
                let first = rope.offset_to_point(range.start).row;
                let end = rope.offset_to_point(range.end).row;
                if range.end > rope.line_end(first) {
                    return first + 1..end + 1;
                }
                row..row
            } else if next && row < last {
                row + 1..row + 2
            } else if !next && row > 0 {
                row..row + 1
            } else {
                row..row
            }
        })
        .collect()
}

/// `spans`, those that are not empty, in order, with those that meet or touch joined.
fn runs_of(spans: &[Range<usize>]) -> Vec<Range<usize>> {
    let mut sorted: Vec<Range<usize>> = spans.iter().filter(|s| !s.is_empty()).cloned().collect();
    sorted.sort_by_key(|span| span.start);
    let mut runs: Vec<Range<usize>> = Vec::new();
    for span in sorted {
        match runs.last_mut() {
            Some(run) if span.start <= run.end => run.end = run.end.max(span.end),
            _ => runs.push(span),
        }
    }
    runs
}

/// One join, or a run of joins over blank lines that meet: what goes, what goes in, and the
/// rows joined to the one before.
struct Join {
    range: Range<usize>,
    space: &'static str,
    rows: Range<usize>,
    /// The last byte of text before it and the first after it, if any.
    around: (Option<u8>, Option<u8>),
}

/// The joins of `runs` of rows, ascending, each row to the one before: the blanks at the end of
/// the row before, the line break and the indentation after it go, and joins that meet over a
/// blank line become one. A run of several rows is read in one pass; a single row, only at the
/// edges of its two lines.
fn joined(rope: &Rope, tab_width: TabWidth, runs: &[Range<usize>]) -> Vec<Join> {
    let mut joins: Vec<Join> = Vec::new();
    let mut leads = Vec::new();
    for run in runs {
        if run.len() == 1 {
            add(&mut joins, edge_join(rope, tab_width, run.start));
            continue;
        }
        leads.clear();
        leads_of(rope, run.start - 1..run.end, tab_width, &mut leads);
        for pair in leads.windows(2) {
            let (before, line) = (pair[0], pair[1]);
            add(
                &mut joins,
                Join {
                    range: before.text_end..line.start + line.bytes,
                    space: "",
                    rows: line.row..line.row + 1,
                    around: (before.last, line.first),
                },
            );
        }
    }
    for join in &mut joins {
        join.space = if spaced(join.around) { " " } else { "" };
    }
    joins
}

/// Adds `join` to `joins`, or joins it with the last one where they meet over a blank line.
fn add(joins: &mut Vec<Join>, join: Join) {
    match joins.last_mut() {
        Some(last) if join.range.start <= last.range.end => {
            last.range.end = join.range.end;
            last.rows.end = join.rows.end;
            last.around.1 = join.around.1;
        }
        _ => joins.push(join),
    }
}

/// The join of `row` to the row before, read at the edges of the two lines, at most
/// [`SCAN`](super::SCAN) bytes at each. A blank a combining mark is on stays, as the mark's base.
fn edge_join(rope: &Rope, tab_width: TabWidth, row: usize) -> Join {
    let start = rope.line_start(row - 1);
    let end = rope.line_end(row - 1);
    let text_end = end - blanks_before(rope, end, start);
    let line = rope.line_start(row);
    let (lead, _) = leading(rope, line, tab_width);
    let kept = lead.min(super::newline::blanks_after(rope, line));
    let after = line + kept;
    Join {
        range: text_end..after,
        space: "",
        rows: row..row + 1,
        around: (
            (text_end > start)
                .then(|| rope.byte_at(text_end - 1))
                .flatten(),
            (after < rope.line_end(row))
                .then(|| rope.byte_at(after))
                .flatten(),
        ),
    }
}

/// Whether a join between `before`, the last byte of text before it, and `after`, the first
/// after it, leaves one space, as `fixup-whitespace` does: not at a line's start or end, after
/// an opening bracket or before a closing one.
fn spaced((before, after): (Option<u8>, Option<u8>)) -> bool {
    let closer = matches!(after, Some(b')' | b']' | b'}'));
    let opener = matches!(before, Some(b'(' | b'[' | b'{'));
    before.is_some() && after.is_some() && !closer && !opener
}

#[cfg(test)]
mod tests;
