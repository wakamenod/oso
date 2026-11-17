//! A side's highlights laid over a review's rows (ADR 0122, step 9): the runs of each line of a
//! side the review shows, taken from the side's document, and put over that line's text in a
//! section, after its sign.
//!
//! Must not depend on: `app`, `dispatch`, git or the file system; this maps runs it is given.
//!
//! - Offsets are the decoded text's, as the section's and the document's are: each invalid byte
//!   a U+FFFD of three, so a line's bytes are the same in both. A BOM, which the document's
//!   text leaves out, takes no highlight on the first line.
//! - A line's text is its bytes without its line break, CR LF too, as the section writes it.
//! - Until the side is highlighted whole, what is not highlighted yet keeps its line's diff
//!   face, so a partial report never shows text as plain that later turns coloured; once it is,
//!   plain text takes the text's colour, as in a buffer (Q-A).
//! - Nothing here panics: a line past the side's end gets nothing, and runs that do not fit a
//!   line's text are cut, a short one ending plain.

use std::ops::Range;

use oso_core::review::Side;
use oso_core::rope::Rope;
use oso_core::syntax::{Highlight, Highlights};

use super::section::Slot;

/// The bytes of a UTF-8 BOM, as a side's first line holds them.
const BOM: &[u8] = b"\xEF\xBB\xBF";

/// A side's runs over the lines the review shows of it, by line number.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Projected {
    /// Each line's number, rising, and where its runs start in `runs`.
    lines: Vec<(u32, usize)>,
    runs: Vec<(u32, Option<Highlight>)>,
    /// Whether the side was highlighted whole.
    pub(super) done: bool,
}

impl Projected {
    /// The runs of line `number`, if it was projected.
    pub(super) fn line(&self, number: u32) -> Option<&[(u32, Option<Highlight>)]> {
        let at = self.lines.binary_search_by_key(&number, |(n, _)| *n).ok()?;
        let start = self.lines[at].1;
        let end = self.lines.get(at + 1).map_or(self.runs.len(), |(_, s)| *s);
        Some(&self.runs[start..end])
    }

    /// How many runs it holds, toward a review's budget.
    pub(super) fn runs(&self) -> usize {
        self.runs.len()
    }

    /// Whether every one of `numbers` was projected.
    pub(super) fn holds_all(&self, mut numbers: impl Iterator<Item = u32>) -> bool {
        numbers.all(|number| self.line(number).is_some())
    }
}

/// Whether `bytes`, a side, starts with a BOM, which its document's text leaves out.
pub(super) fn has_bom(bytes: &[u8]) -> bool {
    bytes.starts_with(BOM)
}

/// The runs of `highlights`, a side's whose text is `text`, over the lines `numbers` (counted
/// from 1), each cut to its line's text. `bom` says the side starts with one, which `text`
/// lacks. O(log n) a line, and a step a run.
pub(super) fn project(
    text: &Rope,
    bom: bool,
    highlights: &Highlights,
    numbers: impl IntoIterator<Item = u32>,
    done: bool,
) -> Projected {
    let mut projected = Projected {
        done,
        ..Projected::default()
    };
    let lines = text.max_point().row;
    let mut last = None;
    for number in numbers {
        let Some(row) = (number as usize).checked_sub(1) else {
            continue;
        };
        // Rising and each once, so a lookup can halve.
        if row > lines || last.is_some_and(|last| number <= last) {
            continue;
        }
        last = Some(number);
        projected.lines.push((number, projected.runs.len()));
        if bom && row == 0 {
            push(&mut projected.runs, BOM.len(), None);
        }
        let span = text.line_start(row)..text.line_end(row);
        for (run, highlight) in highlights.runs(span.clone()) {
            let run = run.start.max(span.start)..run.end.min(span.end);
            push(&mut projected.runs, run.len(), highlight);
        }
    }
    projected
}

fn push(runs: &mut Vec<(u32, Option<Highlight>)>, len: usize, highlight: Option<Highlight>) {
    let Ok(len) = u32::try_from(len) else {
        return;
    };
    if len == 0 {
        return;
    }
    match runs.last_mut() {
        Some((last, kept)) if *kept == highlight => *last += len,
        _ => runs.push((len, highlight)),
    }
}

/// The highlight a line of `side` takes where its side has none: its diff face while the side
/// is still being highlighted, else none, the text's colour.
fn plain(side: Side, done: bool) -> Option<Highlight> {
    if done {
        return None;
    }
    Some(match side {
        Side::Old => Highlight::DiffRemoved,
        Side::New => Highlight::DiffAdded,
    })
}

/// The span of `slot`'s text in a text whose section starts at `start`.
pub(super) fn span(start: usize, slot: &Slot) -> Range<usize> {
    let at = start.saturating_add(slot.at);
    at..at.saturating_add(slot.len)
}

/// Lays `sides`' runs, old then new, over the text of `slots`, a section's that starts at
/// `start` in `highlights`; a slot whose side has no runs for its line keeps its diff face.
pub(super) fn overlay(
    highlights: &mut Highlights,
    start: usize,
    slots: &[Slot],
    sides: [Option<&Projected>; 2],
) {
    for slot in slots {
        let side = match slot.side {
            Side::Old => sides[0],
            Side::New => sides[1],
        };
        let span = span(start, slot);
        match side.and_then(|projected| Some((projected, projected.line(slot.number)?))) {
            Some((projected, runs)) => {
                let plain = plain(slot.side, projected.done);
                let runs = runs
                    .iter()
                    .map(|(len, highlight)| (*len as usize, highlight.or(plain)));
                // A line its side's text does not fill ends plain as well.
                let filled: usize = runs.clone().map(|(len, _)| len).sum();
                let rest = span.len().saturating_sub(filled);
                highlights.replace(span, runs.chain(std::iter::once((rest, plain))));
            }
            None => {
                let diff = plain(slot.side, false);
                highlights.replace(span.clone(), std::iter::once((span.len(), diff)));
            }
        }
    }
}
