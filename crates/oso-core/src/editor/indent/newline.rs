//! `RET` at every cursor of the selected window (ADR 0115): the line break, the spaces and tabs
//! around the cursor that go, the new line's indentation by its mode's rules, and the line it
//! opens between an opener and its closer.
//!
//! Must not depend on: `command` or `keymap`; the commands call this.
//!
//! The text around a cursor is read a little at a time, at most [`SCAN`] bytes, so that a line
//! of a gigabyte of spaces costs no more than a short one.

use std::borrow::Cow;
use std::ops::Range;

use super::{SCAN, blanks_before, text_of};
use crate::buffer::{EditError, Mode, Replacement};
use crate::editor::Editor;
use crate::indent::{self, Answer, Budget, Line, MAX_COLUMN, Settings};
use crate::rope::Rope;
use crate::sum_tree::Bias;

/// What `RET` inserts at one cursor, what it replaces, and how far back from the end of
/// what it inserts the cursor goes.
struct Planned<'a> {
    text: Cow<'a, str>,
    range: Range<usize>,
    back: usize,
}

/// What `RET` does at one cursor.
pub(super) struct Break {
    /// The spaces and tabs before and after the cursor that go.
    pub(super) range: Range<usize>,
    /// Whether the cursor is between an opener and its closer, with only blanks between.
    pub(super) between: bool,
}

impl Editor {
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
        let deepest = if indent {
            2 * (ending.len() + MAX_COLUMN)
        } else {
            0
        };
        let most = ending
            .len()
            .checked_mul(count)
            .and_then(|breaks| breaks.checked_add(deepest))
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
        let mut spots = spots(rope, &cursors, mode);
        keep_hard_breaks(rope, settings, &cursors, &mut spots, mode);
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
/// an opener and its closer, which only Rust, and TOML's arrays, open a line between.
pub(super) fn spots(rope: &Rope, cursors: &[usize], mode: Mode) -> Vec<Break> {
    let mut spots: Vec<Break> = Vec::with_capacity(cursors.len());
    for (index, &at) in cursors.iter().enumerate() {
        let start = rope.line_start(rope.offset_to_point(at).row);
        // The blanks between two cursors go with the first, so no two ranges overlap.
        let floor = spots
            .last()
            .map_or(start, |before: &Break| before.range.end.max(start));
        let ceiling = cursors.get(index + 1).copied().unwrap_or(usize::MAX);
        let before = if at <= floor {
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
        // TOML's inline tables cannot span lines, and `(` means nothing in it.
        let between = match mode {
            Mode::Rust => matches!(
                (opener, closer),
                (Some(b'{'), Some(b'}')) | (Some(b'('), Some(b')')) | (Some(b'['), Some(b']'))
            ),
            Mode::Toml => (opener, closer) == (Some(b'['), Some(b']')),
            _ => false,
        };
        spots.push(Break { range, between });
    }
    spots
}

/// In Markdown, two spaces or more at the end of a line of text break it (ADR 0113 keeps them
/// too): a cursor after them at the line's end, outside a fenced code block, keeps them.
fn keep_hard_breaks(
    rope: &Rope,
    settings: Settings,
    cursors: &[usize],
    spots: &mut [Break],
    mode: Mode,
) {
    if mode != Mode::Markdown {
        return;
    }
    let ends_line = |at: usize| matches!(rope.byte_at(at), None | Some(b'\n' | b'\r'));
    let breaking: Vec<usize> = cursors
        .iter()
        .zip(spots.iter())
        .enumerate()
        .filter_map(|(index, (&at, spot))| {
            let trimmed = at - spot.range.start;
            let spaces = trimmed >= 2
                && text_of(rope, spot.range.start..at)
                    .bytes()
                    .all(|b| b == b' ');
            let text_before = spot.range.start > rope.line_start(rope.offset_to_point(at).row);
            (spaces && text_before && ends_line(spot.range.end)).then_some(index)
        })
        .collect();
    if breaking.is_empty() {
        return;
    }
    let offsets: Vec<usize> = breaking.iter().map(|&index| cursors[index]).collect();
    let fenced = indent::markdown_fenced(rope, settings, &offsets);
    for (index, fenced) in breaking.into_iter().zip(fenced) {
        if !fenced {
            spots[index].range.start = cursors[index];
        }
    }
}

/// The spaces and tabs just after `at`, in bytes, reading at most [`SCAN`] of them. A blank a
/// combining mark is on is the mark's base, not a blank: it stays, with what follows it.
pub(super) fn blanks_after(rope: &Rope, at: usize) -> usize {
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
