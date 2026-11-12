//! Reading the lines of a run of rows in one pass (ADR 0115): each one's indentation, and where
//! its text ends before trailing blanks, for the commands over a region's lines, and what such a
//! read costs.
//!
//! Must not depend on: `command` or `keymap`; it reads a rope.
//!
//! One pass over the text costs a few milliseconds a megabyte, where a seek to each line costs a
//! microsecond or two a line: a region of thirty thousand lines is read in one pass.

use std::ops::Range;

use super::SCAN;
use crate::display_map::TabWidth;
use crate::indent::LINE_COST;
use crate::rope::Rope;

/// What reading `runs` of rows costs, in bytes: each run's bytes, and at least [`LINE_COST`] a
/// line, as the rules' budget counts them.
pub(super) fn read_cost(rope: &Rope, runs: &[Range<usize>]) -> usize {
    runs.iter().fold(0usize, |total, run| {
        let bytes = rope.line_start(run.end) - rope.line_start(run.start);
        let lines = (run.end - run.start).saturating_mul(LINE_COST);
        total.saturating_add(bytes).saturating_add(lines)
    })
}

/// One line as it stands.
#[derive(Clone, Copy, Debug)]
pub(super) struct Lead {
    pub(super) row: usize,
    /// Where the line starts.
    pub(super) start: usize,
    /// The indentation's length in bytes, and the column it reaches.
    pub(super) bytes: usize,
    pub(super) column: usize,
    /// Nothing but spaces and tabs is on the line.
    pub(super) blank: bool,
    /// The indentation is spaces only.
    pub(super) spaces: bool,
    /// The indentation has no tab after a space, so its tabs reach stops from the margin.
    pub(super) tabs_first: bool,
    /// The indentation is longer than [`SCAN`] bytes, of which `bytes` counts the first.
    pub(super) long: bool,
    /// Where the line's text ends, before trailing spaces, tabs and carriage returns: its start
    /// for a blank line.
    pub(super) text_end: usize,
    /// The first byte of its text past the indentation, and the last before `text_end`: none
    /// for a blank line.
    pub(super) first: Option<u8>,
    pub(super) last: Option<u8>,
}

impl Lead {
    fn at(row: usize, start: usize) -> Self {
        Self {
            row,
            start,
            bytes: 0,
            column: 0,
            blank: false,
            spaces: true,
            tabs_first: true,
            long: false,
            text_end: start,
            first: None,
            last: None,
        }
    }
}

/// Every line of `rows`, in order, read in one pass over their text, added to `leads`.
pub(super) fn leads_of(
    rope: &Rope,
    rows: Range<usize>,
    tab_width: TabWidth,
    leads: &mut Vec<Lead>,
) {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum State {
        /// In the indentation.
        Lead,
        /// Past it, at carriage returns only: blank if a line break follows.
        Return,
        /// Past it, to the line's end.
        Rest,
    }
    let from = rope.line_start(rows.start);
    let to = rope.line_start(rows.end);
    let mut lead = Lead::at(rows.start, from);
    let mut state = State::Lead;
    let mut base = from;
    for chunk in rope.chunks_in_range(from..to) {
        let bytes = chunk.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            let byte = bytes[index];
            match (state, byte) {
                (State::Rest, 0..=9 | 11..) => {
                    let segment = &bytes[index..];
                    let end = segment
                        .iter()
                        .position(|&byte| byte == b'\n')
                        .unwrap_or(segment.len());
                    let text = segment[..end]
                        .iter()
                        .rposition(|byte| !matches!(byte, b' ' | b'\t' | b'\r'));
                    if let Some(last) = text {
                        lead.text_end = base + index + last + 1;
                        lead.last = Some(segment[last]);
                    }
                    index += end;
                    continue;
                }
                (_, b'\n') => {
                    keep_base(rope, tab_width, &mut lead);
                    leads.push(Lead {
                        blank: state != State::Rest,
                        ..lead
                    });
                    lead = Lead::at(lead.row + 1, base + index + 1);
                    state = State::Lead;
                }
                (State::Lead, b' ' | b'\t') if lead.bytes == SCAN => {
                    lead.long = true;
                    state = State::Rest;
                    continue;
                }
                (State::Lead, b' ') => {
                    lead.bytes += 1;
                    lead.column = lead.column.saturating_add(1);
                }
                (State::Lead, b'\t') => {
                    // Only tabs so far: each of them reached a stop.
                    let width = tab_width.get() as usize;
                    lead.tabs_first &= lead.bytes * width == lead.column;
                    lead.bytes += 1;
                    lead.column = tab_width.next_stop(lead.column);
                    lead.spaces = false;
                }
                (State::Lead | State::Return, b'\r') => state = State::Return,
                (State::Lead | State::Return, _) => {
                    lead.text_end = base + index + 1;
                    lead.first = Some(byte);
                    lead.last = Some(byte);
                    state = State::Rest;
                }
            }
            index += 1;
        }
        base += bytes.len();
    }
    // The text's last line, with no line break after it.
    if lead.row < rows.end {
        keep_base(rope, tab_width, &mut lead);
        leads.push(Lead {
            blank: state != State::Rest,
            ..lead
        });
    }
}

/// Leaves out of `lead`'s indentation its last blank when a combining mark is on it: the blank
/// is the mark's base, which stays with it, as `RET` keeps it. Only a line whose text starts
/// with a byte past ASCII is read again.
fn keep_base(rope: &Rope, tab_width: TabWidth, lead: &mut Lead) {
    if lead.long || lead.bytes == 0 || lead.first.is_none_or(|byte| byte.is_ascii()) {
        return;
    }
    let text = lead.start + lead.bytes;
    let next = rope.clip_offset((text + 4).min(rope.len()), crate::sum_tree::Bias::Left);
    let marked = rope
        .chunks_in_range(text..next)
        .flat_map(str::chars)
        .next()
        .is_some_and(crate::display_map::cell::is_mark);
    if !marked {
        return;
    }
    lead.bytes -= 1;
    let blanks: String = rope.chunks_in_range(lead.start..text - 1).collect();
    let (_, column) = crate::indent::indentation([blanks.as_str()], tab_width);
    lead.column = column;
    lead.spaces = !blanks.contains('\t');
    lead.tabs_first = !blanks.contains(" \t");
    lead.first = rope.byte_at(text - 1);
}
