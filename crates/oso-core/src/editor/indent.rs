//! Indentation at every cursor of the selected window (ADR 0115): `TAB` reindenting the
//! cursors' lines, `RET` with the new line's indentation, the line `RET` opens between
//! brackets, and the line a typed closer starts. The rules are the buffer's mode's, in
//! `indent`; this applies their answers as one edit.
//!
//! Must not depend on: `command` or `keymap`; the commands call this.
//!
//! The text around a cursor is read a little at a time, at most [`SCAN`] bytes, so that a
//! line of a gigabyte of spaces costs no more than a short one.

use super::Editor;
use crate::buffer::{EditError, Mode, Replacement};
use crate::indent::{self, Answer, Budget, Line, Settings};
use crate::rope::Rope;
use crate::sum_tree::Bias;

/// The most bytes read before or after a cursor for the spaces and tabs around it, or at a
/// line's start for its indentation. Past them, the rules answer no column (`indent`'s own
/// bound on what it reads at a line's start is the same).
const SCAN: usize = 4096;

/// What `RET` inserts at one cursor, what it replaces, and how far back from the end of
/// what it inserts the cursor goes.
struct Planned {
    text: String,
    range: std::ops::Range<usize>,
    back: usize,
}

/// What `RET` does at one cursor.
struct Break {
    /// The spaces and tabs before and after the cursor that go.
    range: std::ops::Range<usize>,
    /// Whether the cursor is between an opener and its closer, with only blanks between.
    between: bool,
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

    /// Sets each mode's indentation, as the configuration gives it, for the buffers visited
    /// or set to a mode after.
    pub fn set_mode_indents(&mut self, indents: [(Mode, Settings); 4]) {
        self.mode_indents = indents;
    }

    /// The indentation of `mode`: the configuration's, or four spaces with tabs eight wide.
    pub fn mode_indent(&self, mode: Mode) -> Settings {
        self.mode_indents
            .iter()
            .find(|(known, _)| *known == mode)
            .map_or_else(Settings::default, |&(_, settings)| settings)
    }

    /// `TAB` (`indent-for-tab-command`): in Rust and TOML, reindents the line of every cursor
    /// of the selected window, each once, as one edit; a cursor inside the indentation goes to
    /// its end, and one in the text keeps its place in it. A line already right does not
    /// change. In Markdown (until its cycle comes), Fundamental mode and internal buffers it
    /// runs [`Editor::tab_to_tab_stop`].
    pub fn indent_for_tab(&mut self) -> Result<(), EditError> {
        let (window, buffer) = self.current();
        // Markdown's `TAB` cycles through its stops in step 3; until then it keeps the stops.
        let language = matches!(buffer.mode(), Mode::Rust | Mode::Toml) && !buffer.is_internal();
        let Some(rules) = buffer.mode().indent_rules().filter(|_| language) else {
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
        self.reindent_rows(rules, &rows)?;
        // A cursor inside its line's indentation goes to the end of it.
        let (window, buffer) = self.current_mut();
        let rope = buffer.text();
        window.selections.update(|selection| {
            let start = rope.line_start(rope.offset_to_point(selection.cursor).row);
            let end = start + leading(rope, start, buffer.tab_width()).bytes;
            selection.cursor = selection.cursor.max(end);
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
        if !self.electric_indent || !matches!(buffer.mode(), Mode::Rust | Mode::Toml) {
            return Ok(());
        }
        let Some(rules) = buffer.mode().indent_rules() else {
            return Ok(());
        };
        let rope = buffer.text();
        let mut rows: Vec<usize> = window
            .selections
            .iter()
            .filter_map(|selection| {
                let at = selection.cursor;
                let closer = at.checked_sub(1).and_then(|before| byte_at(rope, before))?;
                let start = rope.line_start(rope.offset_to_point(at).row);
                let first = matches!(closer, b'}' | b')' | b']')
                    && blanks_before(rope, at - 1, start) == at - 1 - start;
                first.then(|| rope.offset_to_point(at).row)
            })
            .collect();
        rows.dedup();
        if rows.is_empty() {
            return Ok(());
        }
        self.reindent_rows(rules, &rows)
    }

    /// `RET` at every cursor of the selected window: `count` line breaks `ending`, as one
    /// edit. With `indent`, the spaces and tabs around each cursor go and the last new line
    /// takes its rules' indentation; between an opener and its closer, in Rust and TOML, the
    /// closer goes to a line of its own and the cursor to an indented line between. A shown
    /// region is deleted first. Returns false, changing nothing, if more than `limit` bytes
    /// would go in.
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
        let set = &self.window().selections;
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
        let Some(planned) = self.planned_breaks(&breaks, ending, indent) else {
            return Ok(false);
        };
        let total = planned
            .iter()
            .map(|planned| planned.text.len())
            .fold(0usize, usize::saturating_add);
        if total > limit {
            return Ok(false);
        }
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
    /// what it inserts the cursor goes. None if the rules answer fewer lines than asked.
    fn planned_breaks(&self, breaks: &str, ending: &str, indent: bool) -> Option<Vec<Planned>> {
        let (window, buffer) = self.current();
        let rope = buffer.text();
        let settings = buffer.indent_settings();
        let rules = buffer
            .mode()
            .indent_rules()
            .filter(|_| indent && !buffer.is_internal());
        let brackets = matches!(buffer.mode(), Mode::Rust | Mode::Toml);
        let cursors: Vec<usize> = window.selections.iter().map(|s| s.cursor).collect();
        let Some(rules) = rules else {
            let plain = |at| Planned {
                text: breaks.to_owned(),
                range: at..at,
                back: 0,
            };
            return Some(cursors.iter().map(|&at| plain(at)).collect());
        };
        let spots: Vec<Break> = cursors.iter().map(|&at| spot(rope, at, brackets)).collect();
        let mut lines = Vec::with_capacity(cursors.len() * 2);
        for (&at, spot) in cursors.iter().zip(&spots) {
            if spot.between {
                lines.push(Line::Empty(at));
            }
            lines.push(Line::New(at));
        }
        let answers = rules(rope, settings, &lines, &mut Budget::new());
        let mut answers = answers.into_iter();
        let mut all = Vec::with_capacity(cursors.len());
        for (&at, spot) in cursors.iter().zip(&spots) {
            let middle = if spot.between { answers.next() } else { None };
            let last = answers.next()?;
            all.push(planned(breaks, ending, settings, (at, spot), middle, last));
        }
        Some(all)
    }

    /// Reindents `rows` by `rules`, as one edit: each line answered a column it does not
    /// have gets that indentation, in the buffer's style.
    fn reindent_rows(
        &mut self,
        rules: fn(&Rope, Settings, &[Line], &mut Budget) -> Vec<Answer>,
        rows: &[usize],
    ) -> Result<(), EditError> {
        let (window, buffer) = self.current();
        let id = window.buffer();
        let rope = buffer.text();
        let settings = buffer.indent_settings();
        let lines: Vec<_> = rows.iter().map(|&row| Line::Row(row)).collect();
        let answers = rules(rope, settings, &lines, &mut Budget::new());
        let mut texts = Vec::new();
        let mut ranges = Vec::new();
        for (&row, answer) in rows.iter().zip(answers) {
            let Answer::Column(column) = answer else {
                continue;
            };
            let start = rope.line_start(row);
            let now = leading(rope, start, settings.tab_width());
            let wanted = settings.indentation(column);
            if text_of(rope, start..start + now.bytes) != wanted {
                texts.push(wanted);
                ranges.push(start..start + now.bytes);
            }
        }
        if texts.is_empty() {
            return Ok(());
        }
        let edits: Vec<_> = ranges
            .into_iter()
            .zip(&texts)
            .map(|(range, text)| Replacement { range, text })
            .collect();
        self.apply_edits(id, &edits)
    }
}

/// What `RET` inserts at the cursor `at`, given where it is, the answer for the empty line
/// between brackets if it is between them, and the answer for the last new line.
fn planned(
    breaks: &str,
    ending: &str,
    settings: Settings,
    (at, spot): (usize, &Break),
    middle: Option<Answer>,
    last: Answer,
) -> Planned {
    match (middle, last) {
        (Some(Answer::Column(inner)), Answer::Column(outer)) => {
            let closer = settings.indentation(outer);
            let back = ending.len() + closer.len();
            let text = format!("{breaks}{}{ending}{closer}", settings.indentation(inner));
            Planned {
                text,
                range: spot.range.clone(),
                back,
            }
        }
        (_, Answer::Column(column)) => Planned {
            text: format!("{breaks}{}", settings.indentation(column)),
            range: spot.range.clone(),
            back: 0,
        },
        // Inside a string, nothing around the cursor goes and no indentation comes.
        (_, Answer::Keep) => Planned {
            text: breaks.to_owned(),
            range: at..at,
            back: 0,
        },
    }
}

/// Where `RET` breaks the line at `at`: the spaces and tabs around it, and whether it is
/// between an opener and its closer, which `brackets` says to look for.
fn spot(rope: &Rope, at: usize, brackets: bool) -> Break {
    let start = rope.line_start(rope.offset_to_point(at).row);
    let before = blanks_before(rope, at, start);
    let after = blanks_after(rope, at);
    let range = at - before..at + after;
    let opener = range
        .start
        .checked_sub(1)
        .and_then(|before| byte_at(rope, before));
    let closer = byte_at(rope, range.end);
    let between = brackets
        && matches!(
            (opener, closer),
            (Some(b'{'), Some(b'}')) | (Some(b'('), Some(b')')) | (Some(b'['), Some(b']'))
        );
    Break { range, between }
}

/// The byte at `at`, if there is one.
fn byte_at(rope: &Rope, at: usize) -> Option<u8> {
    let end = rope.clip_offset(at + 1, Bias::Right);
    rope.chunks_in_range(at..end)
        .next()?
        .as_bytes()
        .first()
        .copied()
}

/// The spaces and tabs just before `at`, back to `start` at most, in bytes, reading at most
/// [`SCAN`] of them.
fn blanks_before(rope: &Rope, at: usize, start: usize) -> usize {
    let from = rope.clip_offset(at.saturating_sub(SCAN).max(start), Bias::Left);
    let text = text_of(rope, from..at);
    text.bytes()
        .rev()
        .take_while(|&byte| matches!(byte, b' ' | b'\t'))
        .count()
}

/// The spaces and tabs just after `at`, in bytes, reading at most [`SCAN`] of them.
fn blanks_after(rope: &Rope, at: usize) -> usize {
    let to = rope.clip_offset((at + SCAN).min(rope.len()), Bias::Left);
    let text = text_of(rope, at..to);
    text.bytes()
        .take_while(|&byte| matches!(byte, b' ' | b'\t'))
        .count()
}

/// The indentation of the line that starts at `start`, reading at most [`SCAN`] bytes.
fn leading(
    rope: &Rope,
    start: usize,
    tab_width: crate::display_map::TabWidth,
) -> indent::Indentation {
    let to = rope.clip_offset((start + SCAN).min(rope.len()), Bias::Left);
    let text = text_of(rope, start..to);
    let line = text.split('\n').next().unwrap_or_default();
    indent::indentation(line, tab_width)
}

/// The text in `range`, copied.
fn text_of(rope: &Rope, range: std::ops::Range<usize>) -> String {
    rope.chunks_in_range(range).collect()
}

#[cfg(test)]
mod tests;
