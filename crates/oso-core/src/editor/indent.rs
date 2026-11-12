//! Indentation at every cursor of the selected window (ADR 0115): `TAB` reindenting the
//! cursors' lines, and the line a typed closer starts; each mode's indentation, which a buffer
//! takes when it takes the mode; and what the commands in the modules below share: `RET`
//! (`newline`), the region's commands (`region`), a line's (`line`) and Markdown's
//! (`markdown`), over lines read in one pass (`leads`). The rules are the buffer's mode's, in
//! `indent`; these apply their answers as one edit.
//!
//! Must not depend on: `command` or `keymap`; the commands call this.
//!
//! The text around a cursor is read a little at a time, at most [`SCAN`] bytes, so that a
//! line of a gigabyte of spaces costs no more than a short one.

use std::fmt;
use std::ops::Range;

use super::Editor;
use crate::buffer::{Buffer, BufferId, EditError, Mode, Replacement};
use crate::indent::{self, Answer, Budget, Line, MAX_COLUMN, Settings, Style};
use crate::rope::Rope;
use crate::sum_tree::Bias;

/// The most bytes read before or after a cursor for the spaces and tabs around it, or at a
/// line's start for its indentation. Past them, the rules answer no column (`indent`'s own
/// bound on what it reads at a line's start is the same).
const SCAN: usize = 4096;

mod leads;
mod line;
mod markdown;
mod newline;
mod region;

use newline::spots;
pub(super) use region::RigidLayer;
pub use region::Shift;

/// Why an indentation command changed nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndentError {
    /// A cursor has no mark, so no region.
    NoMark,
    /// The regions hold more than one command may reindent on the UI thread.
    TooLarge,
    /// The buffer's mode has no rules to reindent lines by.
    NoRules,
    /// The cursor is in no Markdown list item.
    NotInList,
    /// More text would go in than one command may insert.
    TooMuchText,
    /// The edit failed.
    Edit(EditError),
}

impl fmt::Display for IndentError {
    /// The message Emacs shows, where it has one.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoMark => f.write_str("The mark is not set now"),
            Self::TooLarge => f.write_str("Region too large to indent"),
            Self::NoRules => f.write_str("No indentation rules in this mode"),
            Self::NotInList => f.write_str("Not in a list"),
            Self::TooMuchText => f.write_str("Too much text to insert"),
            Self::Edit(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for IndentError {}

impl From<EditError> for IndentError {
    fn from(error: EditError) -> Self {
        Self::Edit(error)
    }
}

/// The rules of a mode, as `Mode::indent_rules` gives them.
type Rules = fn(&Rope, Settings, &[Line], &mut Budget) -> Vec<Answer>;

/// The rules `TAB`, `indent-region` and a typed closer reindent a buffer's lines by: Rust's and
/// TOML's, in a buffer of the person's. Markdown's `TAB` cycles through its stops instead, and
/// Fundamental mode keeps the tab stops.
fn reindenting(buffer: &Buffer) -> Option<Rules> {
    let language = matches!(buffer.mode(), Mode::Rust | Mode::Toml) && !buffer.is_internal();
    buffer.mode().indent_rules().filter(|_| language)
}

impl Editor {
    /// Whether `RET` indents the line it starts (`electric-indent-mode`, ADR 0115): on by
    /// default.
    pub fn electric_indent(&self) -> bool {
        self.electric_indent
    }

    /// Turns `RET`'s indentation and the typed closers' on or off.
    pub fn set_electric_indent(&mut self, on: bool) {
        self.electric_indent = on;
    }

    /// Sets each mode's indentation, as the configuration gives it, and gives every buffer of
    /// the person's its mode's indent size and style. A buffer's tab width stays the one it was
    /// visited with, which its display map is built with.
    pub fn set_mode_indents(&mut self, indents: [(Mode, Settings); 4]) {
        self.mode_indents = indents;
        let ids: Vec<BufferId> = self.buffers().map(|(id, _)| id).collect();
        for id in ids {
            self.give_mode_indent(id);
        }
    }

    /// The indentation of `mode`: the configuration's, or four spaces with tabs eight wide.
    pub fn mode_indent(&self, mode: Mode) -> Settings {
        self.mode_indents
            .iter()
            .find(|(known, _)| *known == mode)
            .map_or_else(Settings::default, |&(_, settings)| settings)
    }

    /// Gives the buffer `id` its mode's indent size and style, unless it is internal: as a
    /// visit, a mode command or a new name that changes its mode does.
    pub(crate) fn give_mode_indent(&mut self, id: BufferId) {
        let Some(mode) = self
            .buffer(id)
            .filter(|b| !b.is_internal())
            .map(Buffer::mode)
        else {
            return;
        };
        let settings = self.mode_indent(mode);
        if let Some(buffer) = self.buffer_mut(id) {
            buffer.set_indent_settings(settings);
        }
    }

    /// `TAB` (`indent-for-tab-command`): in Rust and TOML, reindents the line of every cursor
    /// of the selected window, each once, as one edit; on a line reindented, a cursor inside
    /// the indentation goes to its end, and one in the text keeps its place in it. A line
    /// already right does not change. In Markdown it cycles each line through the columns its
    /// list gives it. In Fundamental mode and internal buffers it runs
    /// [`Editor::tab_to_tab_stop`].
    pub fn indent_for_tab(&mut self) -> Result<(), EditError> {
        let (window, buffer) = self.current();
        let Some(rules) = reindenting(buffer) else {
            if buffer.mode() == Mode::Markdown && !buffer.is_internal() {
                return self.markdown_tab();
            }
            return self.tab_to_tab_stop();
        };
        if let Some(reason) = buffer.read_only() {
            return Err(EditError::ReadOnly(reason));
        }
        let rope = buffer.text();
        let mut rows: Vec<usize> = window
            .selections
            .iter()
            .map(|selection| rope.offset_to_point(selection.cursor).row)
            .collect();
        rows.dedup();
        let answered = self.reindent_rows(rules, &rows)?;
        // A cursor inside the indentation of a line answered goes to the end of it; one in a
        // string the rules keep stays where it is.
        self.cursors_past_indentation(&answered);
        Ok(())
    }

    /// Moves each cursor of the selected window on one of `rows`, ascending, that is inside its
    /// line's indentation to the end of it.
    fn cursors_past_indentation(&mut self, rows: &[usize]) {
        let (window, buffer) = self.current_mut();
        let rope = buffer.text();
        window.selections.update(|selection| {
            let row = rope.offset_to_point(selection.cursor).row;
            if rows.binary_search(&row).is_ok() {
                let start = rope.line_start(row);
                let end = start + leading(rope, start, buffer.tab_width()).0;
                selection.cursor = selection.cursor.max(end);
            }
            Ok::<(), ()>(())
        });
    }

    /// After a typed `}`, `)` or `]` (electric closers, ADR 0115): with
    /// `electric-indent-mode` on, in Rust and TOML, reindents each cursor's line whose first
    /// non-blank is the closer just before the cursor, so the closer lands under its
    /// opener's line. The typing command calls it, so the change undoes with the typing.
    pub fn indent_typed_closers(&mut self) -> Result<(), EditError> {
        let (window, buffer) = self.current();
        let Some(rules) = reindenting(buffer).filter(|_| self.electric_indent) else {
            return Ok(());
        };
        let rope = buffer.text();
        let mut rows: Vec<usize> = window
            .selections
            .iter()
            .filter_map(|selection| {
                let at = selection.cursor;
                let closer = rope.byte_at(at.checked_sub(1)?)?;
                let row = rope.offset_to_point(at).row;
                let start = rope.line_start(row);
                let first = matches!(closer, b'}' | b')' | b']')
                    && blanks_before(rope, at - 1, start) == at - 1 - start;
                first.then_some(row)
            })
            .collect();
        rows.dedup();
        if rows.is_empty() {
            return Ok(());
        }
        self.reindent_rows(rules, &rows).map(|_| ())
    }

    /// Reindents `rows` by `rules`, as one edit: each line answered a column it does not
    /// have gets that indentation, in the buffer's style. Returns the rows answered a column,
    /// in order.
    fn reindent_rows(&mut self, rules: Rules, rows: &[usize]) -> Result<Vec<usize>, EditError> {
        let (window, buffer) = self.current();
        let id = window.buffer();
        let rope = buffer.text();
        let settings = buffer.indent_settings();
        let lines: Vec<_> = rows.iter().map(|&row| Line::Row(row)).collect();
        let answers = rules(rope, settings, &lines, &mut Budget::new());
        let mut answered = Vec::with_capacity(rows.len());
        let mut texts = Vec::new();
        let mut ranges = Vec::new();
        for (&row, answer) in rows.iter().zip(answers) {
            let Answer::Column(column) = answer else {
                continue;
            };
            answered.push(row);
            let start = rope.line_start(row);
            let (bytes, now) = leading(rope, start, settings.tab_width());
            if has_indentation(rope, start..start + bytes, now, column, settings) {
                continue;
            }
            texts.push(settings.indentation(column));
            ranges.push(start..start + bytes);
        }
        if !texts.is_empty() {
            let edits: Vec<_> = ranges
                .into_iter()
                .zip(&texts)
                .map(|(range, text)| Replacement { range, text })
                .collect();
            self.apply_edits(id, &edits)?;
        }
        Ok(answered)
    }
}

/// Whether the indentation in `range`, which reaches `now`, already is the one that reaches
/// `column` in `settings`' style. Spaces, or tabs then spaces, are as long as what they reach
/// says: only an indentation that reaches the column in as many bytes is read to compare.
fn has_indentation(
    rope: &Rope,
    range: Range<usize>,
    now: usize,
    column: usize,
    settings: Settings,
) -> bool {
    now == column
        && range.len() == indentation_bytes(settings, column)
        && text_of(rope, range) == settings.indentation(column)
}

/// The length in bytes of the indentation that reaches `column` in `settings`' style.
fn indentation_bytes(settings: Settings, column: usize) -> usize {
    match settings.style() {
        Style::Spaces => column,
        Style::Tabs => {
            let width = settings.tab_width().get() as usize;
            column / width + column % width
        }
    }
}

/// The spaces and tabs just before `at`, back to `start` at most, in bytes, reading at most
/// [`SCAN`] of them, a little first: most cursors follow text.
fn blanks_before(rope: &Rope, at: usize, start: usize) -> usize {
    let blank = |byte| matches!(byte, b' ' | b'\t');
    let follows_blank = at
        .checked_sub(1)
        .and_then(|before| rope.byte_at(before))
        .is_some_and(blank);
    if at <= start || !follows_blank {
        return 0;
    }
    let mut back = 64;
    loop {
        let from = rope.clip_offset(at.saturating_sub(back).max(start), Bias::Left);
        let text = text_of(rope, from..at);
        let blanks = text.bytes().rev().take_while(|&byte| blank(byte)).count();
        if blanks < text.len() || from == start || back >= SCAN {
            return blanks;
        }
        back = (back * 8).min(SCAN);
    }
}

/// The indentation of the line that starts at `start`, reading at most [`SCAN`] bytes: its
/// length in bytes and the column it reaches, as the rules count it.
fn leading(rope: &Rope, start: usize, tab_width: crate::display_map::TabWidth) -> (usize, usize) {
    let to = rope.clip_offset((start + SCAN).min(rope.len()), Bias::Left);
    indent::indentation(rope.chunks_in_range(start..to), tab_width)
}

/// The text in `range`, copied.
fn text_of(rope: &Rope, range: Range<usize>) -> String {
    rope.chunks_in_range(range).collect()
}

#[cfg(test)]
mod tests;
