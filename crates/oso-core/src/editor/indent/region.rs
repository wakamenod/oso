//! The lines of each cursor's region (ADR 0115): `indent-region`, which reindents them by the
//! buffer's mode's rules or puts them at one column, and `indent-rigidly`, which shifts them,
//! with the layer of `C-x TAB` whose keys join one undo step. Each changed line is one
//! replacement of its indentation, in one edit; a line already right is not touched.
//!
//! Must not depend on: `command` or `keymap`; the commands call this, and the dispatcher
//! opens and closes the layer.
//!
//! A region's lines are those Emacs takes: every line that starts before its end, from its start's
//! line for the rules, and from the first line that starts in it for a column or a shift, so an
//! empty region has none. Overlapping regions' lines count once. What the regions cover is checked
//! against [`REGION_LIMIT`] before anything is read line by line.

use std::ops::Range;

use super::leads::{Lead, leads_of, read_cost};
use super::{IndentError, MAX_COLUMN, indentation_bytes, reindenting};
use crate::buffer::{BufferId, ContentId, EditError, Replacement};
use crate::editor::Editor;
use crate::indent::{Answer, Budget, LINE_LOOKBACK, Line, Settings, Style};
use crate::rope::Rope;
use crate::selection::SelectionSet;
use crate::window::WindowId;

/// What one region command may cover, in bytes of its budget: `bytes + lines * LINE_COST` over
/// its lines, plus one lookback. Measured with `text_perf -- indent` so that `indent-region`
/// over it, the edit included, stays under 50 ms (ADR 0115, step 3).
pub(crate) const REGION_LIMIT: usize = 6 << 20;

/// The most lines one region command changes: each is a splice of the edit, which costs about a
/// microsecond (ADR 0092), so that the edit stays within what the reading leaves of 50 ms.
pub(crate) const MAX_CHANGED_LINES: usize = 30_000;

/// Which way `indent-rigidly` goes to a tab stop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shift {
    /// To the previous stop, never past column 0.
    Left,
    /// To the next stop.
    Right,
}

/// The layer `C-x TAB` opens: the window and buffer it shifts in, the undo state and cursors
/// before its first key, which its keys join into one step when it closes, and the undo state its
/// last key left, so that nothing else's edit is joined.
#[derive(Clone, Debug)]
pub(in crate::editor) struct RigidLayer {
    window: WindowId,
    buffer: BufferId,
    began: (ContentId, u64),
    before: SelectionSet,
    after: (ContentId, u64),
}

/// How the lines of a region change.
#[derive(Clone, Copy)]
enum How {
    /// Each by the mode's rules.
    Rules(super::Rules),
    /// Each to this column.
    Column(usize),
    /// Each by this many columns.
    By(isize),
    /// Each by as much as takes the least indented line of its run to a stop of the setting's
    /// indent size.
    ToStop(Shift),
}

impl Editor {
    /// `indent-region` (`C-M-\`): reindents every line of each cursor's region, shown or not,
    /// by the buffer's mode's rules, or with `column` puts each at that column, at most
    /// [`MAX_COLUMN`](crate::indent::MAX_COLUMN). A line of only spaces and tabs becomes empty. Each
    /// line changed is one replacement in one edit, which inserts at most `limit` bytes.
    ///
    /// # Errors
    ///
    /// Nothing changes: a cursor with no mark, a read-only buffer, no rules in its mode with no
    /// `column`, regions past what one command may reindent, or more than `limit` to insert.
    pub fn indent_region(
        &mut self,
        column: Option<usize>,
        limit: usize,
    ) -> Result<(), IndentError> {
        let how = match column {
            Some(column) => How::Column(column.min(MAX_COLUMN)),
            None => How::Rules(reindenting(self.current().1).ok_or(IndentError::NoRules)?),
        };
        self.change_regions(how, limit)
    }

    /// `indent-rigidly` with a count: shifts every line of each cursor's region by `columns`,
    /// to the left when negative, never past column 0 nor to the right past
    /// [`MAX_COLUMN`](crate::indent::MAX_COLUMN). A line of only spaces and tabs becomes empty. While
    /// the layer of `C-x TAB` is open, the region stays as shown as it was.
    ///
    /// # Errors
    ///
    /// As [`Editor::indent_region`], but for the rules.
    pub fn indent_rigidly(&mut self, columns: isize, limit: usize) -> Result<(), IndentError> {
        self.change_regions(How::By(columns), limit)
    }

    /// `indent-rigidly-right-to-tab-stop` and `-left-`: shifts the lines of each region so that
    /// its least indented line goes to the next, or previous, multiple of the buffer's indent
    /// size, as [`Editor::indent_rigidly`].
    ///
    /// # Errors
    ///
    /// As [`Editor::indent_rigidly`].
    pub fn indent_rigidly_to_tab_stop(
        &mut self,
        shift: Shift,
        limit: usize,
    ) -> Result<(), IndentError> {
        self.change_regions(How::ToStop(shift), limit)
    }

    /// Whether the layer of `C-x TAB` is open in the selected window, on the buffer it shows,
    /// whose arrows shift the region. A layer left behind in another window or buffer is not:
    /// the next command closes it.
    pub fn indent_rigidly_layer(&self) -> bool {
        let (window, _) = self.current();
        self.rigid
            .as_ref()
            .is_some_and(|layer| layer.window == window.id() && layer.buffer == window.buffer())
    }

    /// Opens or closes the layer of `C-x TAB`. Opening it notes the undo state; closing it
    /// joins what its keys changed into one undo step, which goes back to the cursors before it,
    /// unless something else changed the buffer since its last key.
    pub fn set_indent_rigidly_layer(&mut self, open: bool) {
        if !open {
            if let Some(layer) = self.rigid.take()
                && let Some(buffer) = self.buffer_mut(layer.buffer)
                && buffer.undo_mark() == layer.after
            {
                buffer.join_undo_since(layer.began, Some(layer.before));
            }
            return;
        }
        if !self.indent_rigidly_layer() {
            self.set_indent_rigidly_layer(false);
            let (window, buffer) = self.current();
            let mark = buffer.undo_mark();
            self.rigid = Some(RigidLayer {
                window: window.id(),
                buffer: window.buffer(),
                began: mark,
                before: window.selections.clone(),
                after: mark,
            });
        }
    }

    /// Changes the lines of every region as `how` says, as one edit.
    fn change_regions(&mut self, how: How, limit: usize) -> Result<(), IndentError> {
        let (window, buffer) = self.current();
        let set = &window.selections;
        if set.iter().any(|selection| selection.mark.is_none()) {
            return Err(IndentError::NoMark);
        }
        if let Some(reason) = buffer.read_only() {
            return Err(EditError::ReadOnly(reason).into());
        }
        let rope = buffer.text();
        let rules = matches!(how, How::Rules(_));
        let runs = region_runs(rope, set, rules);
        let lookback = if rules { LINE_LOOKBACK } else { 0 };
        if read_cost(rope, &runs).saturating_add(lookback) > REGION_LIMIT {
            return Err(IndentError::TooLarge);
        }
        let settings = buffer.indent_settings();
        let mut leads = Vec::new();
        let mut bounds = Vec::with_capacity(runs.len());
        for run in &runs {
            let first = leads.len();
            leads_of(rope, run.clone(), settings.tab_width(), &mut leads);
            // A line whose indentation is longer than the scan reads is left as it is.
            let mut read = leads.split_off(first);
            read.retain(|lead| !lead.long);
            leads.append(&mut read);
            bounds.push(first..leads.len());
        }
        let columns = targets(rope, settings, how, &leads, &bounds)?;
        // Every new indentation, one after another in one string.
        let mut texts = String::new();
        let mut changes: Vec<(Range<usize>, Range<usize>)> = Vec::new();
        for (lead, column) in leads.iter().zip(columns) {
            let Some(column) = column else {
                continue;
            };
            if is_right(lead, column, settings) {
                continue;
            }
            let from = texts.len();
            if !lead.blank {
                push_indentation(&mut texts, settings, column);
            }
            if texts.len() > limit {
                return Err(IndentError::TooMuchText);
            }
            if changes.len() == MAX_CHANGED_LINES {
                return Err(IndentError::TooLarge);
            }
            changes.push((lead.start..lead.start + lead.bytes, from..texts.len()));
        }
        if changes.is_empty() {
            return Ok(());
        }
        let shown = set.mark_active();
        let id = window.buffer();
        let edits: Vec<_> = changes
            .into_iter()
            .map(|(range, text)| Replacement {
                range,
                text: &texts[text],
            })
            .collect();
        self.apply_edits(id, &edits)?;
        if self.indent_rigidly_layer() {
            self.current_mut().0.selections.set_mark_active(shown);
            let mark = self.current().1.undo_mark();
            if let Some(layer) = &mut self.rigid {
                layer.after = mark;
            }
        }
        Ok(())
    }
}

/// The rows of every region of `set`, in order, as runs of rows with overlapping and touching
/// ones joined: every row that starts before a region's end, from its start's row if `partial`,
/// and otherwise from the first row that starts in it, as Emacs's `indent-region` and
/// `indent-rigidly` take them. An empty region has none.
fn region_runs(rope: &Rope, set: &SelectionSet, partial: bool) -> Vec<Range<usize>> {
    let mut rows: Vec<Range<usize>> = set
        .iter()
        .filter_map(|selection| {
            let range = selection.range();
            let mut first = rope.offset_to_point(range.start).row;
            if !partial && range.start > rope.line_start(first) {
                first += 1;
            }
            let last = rope.offset_to_point(range.end).row;
            let end = if range.end > rope.line_start(last) {
                last + 1
            } else {
                last
            };
            (!range.is_empty() && first < end).then_some(first..end)
        })
        .collect();
    rows.sort_by_key(|rows| rows.start);
    let mut runs: Vec<Range<usize>> = Vec::with_capacity(rows.len());
    for next in rows {
        match runs.last_mut() {
            Some(run) if next.start <= run.end => run.end = run.end.max(next.end),
            _ => runs.push(next),
        }
    }
    runs
}

/// Whether `lead` already is the indentation `column` makes in `settings`, or for a blank line,
/// whether it is empty. It reads nothing: the scan noted what the indentation is made of.
fn is_right(lead: &Lead, column: usize, settings: Settings) -> bool {
    if lead.blank {
        return lead.bytes == 0;
    }
    let made = match settings.style() {
        Style::Spaces => lead.spaces,
        Style::Tabs => lead.tabs_first,
    };
    made && lead.column == column && lead.bytes == indentation_bytes(settings, column)
}

/// Adds to `texts` the indentation that reaches `column` in `settings`' style.
fn push_indentation(texts: &mut String, settings: Settings, column: usize) {
    let (tabs, spaces) = match settings.style() {
        Style::Spaces => (0, column),
        Style::Tabs => {
            let width = settings.tab_width().get() as usize;
            (column / width, column % width)
        }
    };
    texts.extend(std::iter::repeat_n('\t', tabs));
    texts.extend(std::iter::repeat_n(' ', spaces));
}

/// The column each of `leads`, the regions' lines in order, goes to as `how` says, or none for
/// a line left as it is; `bounds` are the leads of each run. A blank line's column does not
/// count: it is emptied, unless the rules keep it, as inside a string. All the runs share one
/// pass of the rules and one budget, and a pass that runs out of it changes nothing.
fn targets(
    rope: &Rope,
    settings: Settings,
    how: How,
    leads: &[Lead],
    bounds: &[Range<usize>],
) -> Result<Vec<Option<usize>>, IndentError> {
    let by = |lead: &Lead, shift: isize| {
        let column = lead.column.saturating_add_signed(shift);
        // Never past the deepest column the rules answer, unless the line is there already.
        Some(column.min(MAX_COLUMN.max(lead.column)))
    };
    match how {
        How::Column(column) => Ok(vec![Some(column); leads.len()]),
        How::By(shift) => Ok(leads.iter().map(|lead| by(lead, shift)).collect()),
        How::ToStop(toward) => {
            let size = settings.indent_size();
            let mut columns = Vec::with_capacity(leads.len());
            for bound in bounds {
                let run = &leads[bound.clone()];
                let least = run
                    .iter()
                    .filter(|lead| !lead.blank)
                    .map(|l| l.column)
                    .min();
                let shift = least.map_or(0, |least| {
                    let stop = match toward {
                        Shift::Right => (least / size + 1) * size,
                        Shift::Left => least.saturating_sub(1) / size * size,
                    };
                    signed(stop) - signed(least)
                });
                columns.extend(run.iter().map(|lead| by(lead, shift)));
            }
            Ok(columns)
        }
        How::Rules(rules) => {
            // Blank lines are asked too, so that the pass reads on from each line to the next.
            let lines: Vec<Line> = leads.iter().map(|lead| Line::Row(lead.row)).collect();
            let mut budget = Budget::with_limit(REGION_LIMIT);
            let answers = rules(rope, settings, &lines, &mut budget);
            if budget.is_spent() {
                return Err(IndentError::TooLarge);
            }
            Ok(answers
                .into_iter()
                .map(|answer| match answer {
                    Answer::Column(column) => Some(column),
                    Answer::Keep => None,
                })
                .collect())
        }
    }
}

/// `columns` as a signed number; a column fits, as no line is that wide.
fn signed(columns: usize) -> isize {
    isize::try_from(columns).unwrap_or(isize::MAX)
}

#[cfg(test)]
mod tests;
