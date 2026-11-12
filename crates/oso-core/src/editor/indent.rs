//! Indentation at every cursor of the selected window (ADR 0115): `TAB` reindenting the
//! cursors' lines, `RET` with the new line's indentation, the line `RET` opens between
//! brackets, and the line a typed closer starts; and each mode's indentation, which a buffer
//! takes when it takes the mode. The rules are the buffer's mode's, in `indent`; this applies
//! their answers as one edit.
//!
//! Must not depend on: `command` or `keymap`; the commands call this.
//!
//! The text around a cursor is read a little at a time, at most [`SCAN`] bytes, so that a
//! line of a gigabyte of spaces costs no more than a short one.

use std::borrow::Cow;
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

/// The rules of a mode, as `Mode::indent_rules` gives them.
type Rules = fn(&Rope, Settings, &[Line], &mut Budget) -> Vec<Answer>;

/// What `RET` inserts at one cursor, what it replaces, and how far back from the end of
/// what it inserts the cursor goes.
struct Planned<'a> {
    text: Cow<'a, str>,
    range: Range<usize>,
    back: usize,
}

/// What `RET` does at one cursor.
struct Break {
    /// The spaces and tabs before and after the cursor that go.
    range: Range<usize>,
    /// Whether the cursor is between an opener and its closer, with only blanks between.
    between: bool,
}

/// The rules `TAB` and a typed closer reindent a buffer's lines by: Rust's and TOML's, in a
/// buffer of the person's. Markdown's `TAB` cycles through its stops in step 3; until then it
/// keeps the tab stops, as Fundamental mode does.
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
    /// already right does not change. In Markdown (until its cycle comes), Fundamental mode and
    /// internal buffers it runs [`Editor::tab_to_tab_stop`].
    pub fn indent_for_tab(&mut self) -> Result<(), EditError> {
        let (window, buffer) = self.current();
        let Some(rules) = reindenting(buffer) else {
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
        let (window, buffer) = self.current_mut();
        let rope = buffer.text();
        window.selections.update(|selection| {
            let row = rope.offset_to_point(selection.cursor).row;
            if answered.binary_search(&row).is_ok() {
                let start = rope.line_start(row);
                let end = start + leading(rope, start, buffer.tab_width()).0;
                selection.cursor = selection.cursor.max(end);
            }
            Ok::<(), ()>(())
        });
        Ok(())
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

    /// `RET` at every cursor of the selected window: `count` line breaks `ending`, as one
    /// edit. With `indent`, the spaces and tabs around each cursor go (in Markdown, only those
    /// after it, as two at a line's end break it) and the last new line takes its rules'
    /// indentation; between an opener and its closer, in Rust and TOML, the closer goes to a
    /// line of its own and the cursor to an indented line between. A shown region is deleted
    /// first. Returns false, changing nothing, if more than `limit` bytes could go in.
    pub fn newline(
        &mut self,
        ending: &str,
        count: usize,
        indent: bool,
        limit: usize,
    ) -> Result<bool, EditError> {
        let id = self.window().buffer();
        if let Some(reason) = self.current().1.read_only() {
            return Err(EditError::ReadOnly(reason));
        }
        // What could go in at most, each cursor's indentation at its deepest, twice between
        // brackets: checked before anything changes, the region too.
        let set = &self.window().selections;
        let most = ending
            .len()
            .checked_mul(count)
            .and_then(|breaks| breaks.checked_add(2 * (ending.len() + MAX_COLUMN)))
            .and_then(|each| each.checked_mul(set.len()));
        if most.is_none_or(|most| most > limit) {
            return Ok(false);
        }
        if set.mark_active().is_active() && set.iter().any(|s| !s.range().is_empty()) {
            let deletes: Vec<_> = set
                .iter()
                .map(|selection| Replacement {
                    range: selection.range(),
                    text: "",
                })
                .collect();
            self.apply_edits(id, &deletes)?;
        }
        let breaks = ending.repeat(count);
        let planned = self.planned_breaks(&breaks, ending, indent);
        let edits: Vec<_> = planned
            .iter()
            .map(|planned| Replacement {
                range: planned.range.clone(),
                text: &planned.text,
            })
            .collect();
        self.apply_edits(id, &edits)?;
        // Between brackets, the cursor goes back over the closer's new line.
        let backs: Vec<usize> = planned.iter().map(|planned| planned.back).collect();
        let window = &mut self.windows[self.selected];
        if backs.len() == window.selections.len() && backs.iter().any(|&back| back > 0) {
            let mut backs = backs.into_iter();
            window.selections.update(|selection| {
                selection.cursor -= backs.next().unwrap_or(0);
                Ok::<(), ()>(())
            });
        }
        Ok(true)
    }

    /// For each cursor, what `RET` inserts and replaces, and how far back from the end of
    /// what it inserts the cursor goes.
    fn planned_breaks<'a>(&self, breaks: &'a str, ending: &str, indent: bool) -> Vec<Planned<'a>> {
        let (window, buffer) = self.current();
        let rope = buffer.text();
        let settings = buffer.indent_settings();
        let mode = buffer.mode();
        let cursors: Vec<usize> = window.selections.iter().map(|s| s.cursor).collect();
        let plain = |at| Planned {
            text: Cow::Borrowed(breaks),
            range: at..at,
            back: 0,
        };
        let rules = mode
            .indent_rules()
            .filter(|_| indent && !buffer.is_internal());
        let Some(rules) = rules else {
            return cursors.iter().map(|&at| plain(at)).collect();
        };
        let spots = spots(rope, &cursors, mode);
        let mut lines = Vec::with_capacity(cursors.len() * 2);
        for (&at, spot) in cursors.iter().zip(&spots) {
            if spot.between {
                lines.push(Line::Empty(at));
            }
            lines.push(Line::New(at));
        }
        let mut answers = rules(rope, settings, &lines, &mut Budget::new()).into_iter();
        let mut all = Vec::with_capacity(cursors.len());
        for (&at, spot) in cursors.iter().zip(&spots) {
            let middle = if spot.between { answers.next() } else { None };
            // The rules answer every line asked; were one missing, a plain break.
            let planned = match answers.next() {
                Some(last) => planned(breaks, ending, settings, (at, spot), middle, last),
                None => plain(at),
            };
            all.push(planned);
        }
        all
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
            // Spaces, or tabs then spaces, are as long as what they reach says: only a line
            // that reaches the column in as many bytes is read again to compare.
            if now == column
                && bytes == indentation_bytes(settings, column)
                && text_of(rope, start..start + bytes) == settings.indentation(column)
            {
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

/// What `RET` inserts at the cursor `at`, given where it is, the answer for the empty line
/// between brackets if it is between them, and the answer for the last new line.
fn planned<'a>(
    breaks: &'a str,
    ending: &str,
    settings: Settings,
    (at, spot): (usize, &Break),
    middle: Option<Answer>,
    last: Answer,
) -> Planned<'a> {
    match (middle, last) {
        (Some(Answer::Column(inner)), Answer::Column(outer)) => {
            let closer = settings.indentation(outer);
            let back = ending.len() + closer.len();
            let text = format!("{breaks}{}{ending}{closer}", settings.indentation(inner));
            Planned {
                text: Cow::Owned(text),
                range: spot.range.clone(),
                back,
            }
        }
        (_, Answer::Column(column)) => Planned {
            text: Cow::Owned(format!("{breaks}{}", settings.indentation(column))),
            range: spot.range.clone(),
            back: 0,
        },
        // Inside a string, nothing around the cursor goes and no indentation comes.
        (_, Answer::Keep) => Planned {
            text: Cow::Borrowed(breaks),
            range: at..at,
            back: 0,
        },
    }
}

/// Where `RET` breaks the line at each of `cursors`, in order, in a buffer in `mode`: the
/// spaces and tabs around it, no further than the cursors beside it, and whether it is between
/// an opener and its closer, which only Rust and TOML open a line between.
fn spots(rope: &Rope, cursors: &[usize], mode: Mode) -> Vec<Break> {
    let brackets = matches!(mode, Mode::Rust | Mode::Toml);
    // In Markdown, two spaces at a line's end break it: they stay (ADR 0113 keeps them too).
    let keep_before = mode == Mode::Markdown;
    let mut spots: Vec<Break> = Vec::with_capacity(cursors.len());
    for (index, &at) in cursors.iter().enumerate() {
        let start = rope.line_start(rope.offset_to_point(at).row);
        // The blanks between two cursors go with the first, so no two ranges overlap.
        let floor = spots
            .last()
            .map_or(start, |before: &Break| before.range.end.max(start));
        let ceiling = cursors.get(index + 1).copied().unwrap_or(usize::MAX);
        let before = if keep_before || at <= floor {
            0
        } else {
            blanks_before(rope, at, floor)
        };
        let after = blanks_after(rope, at).min(ceiling.saturating_sub(at));
        let range = at - before..at + after;
        let opener = range
            .start
            .checked_sub(1)
            .and_then(|before| rope.byte_at(before));
        let closer = rope.byte_at(range.end);
        let between = brackets
            && matches!(
                (opener, closer),
                (Some(b'{'), Some(b'}')) | (Some(b'('), Some(b')')) | (Some(b'['), Some(b']'))
            );
        spots.push(Break { range, between });
    }
    spots
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

/// The spaces and tabs just after `at`, in bytes, reading at most [`SCAN`] of them. A blank a
/// combining mark is on is the mark's base, not a blank: it stays, with what follows it.
fn blanks_after(rope: &Rope, at: usize) -> usize {
    let run = blank_run_after(rope, at);
    let next = rope.clip_offset(at + run + 4, Bias::Left);
    let marked = text_of(rope, at + run..next)
        .chars()
        .next()
        .is_some_and(crate::display_map::cell::is_mark);
    if marked { run.saturating_sub(1) } else { run }
}

/// The spaces and tabs just after `at`, in bytes, reading at most [`SCAN`] of them.
fn blank_run_after(rope: &Rope, at: usize) -> usize {
    let to = rope.clip_offset((at + SCAN).min(rope.len()), Bias::Left);
    let mut blanks = 0;
    for chunk in rope.chunks_in_range(at..to) {
        let run = chunk
            .bytes()
            .take_while(|&byte| matches!(byte, b' ' | b'\t'))
            .count();
        blanks += run;
        if run < chunk.len() {
            break;
        }
    }
    blanks
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
