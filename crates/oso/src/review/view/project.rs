//! A side's highlights laid over a review's rows (ADR 0122, step 9): the runs of each line of a
//! side the review shows, taken from the side's document, and merged with a section's own runs
//! over that line's text, after its sign.
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
//! - A section's runs are merged in one pass over its own runs and its slots, so a text laid
//!   out costs its runs, and a paint one replace a section.
//! - Nothing here panics: a line past the side's end gets nothing, and runs that do not fit a
//!   line's text are cut, a short one ending plain.

use oso_core::decode::BOM;
use oso_core::review::Side;
use oso_core::rope::Rope;
use oso_core::syntax::{Highlight, Highlights};

use super::section::{Rendered, Slot};

/// A run of a section: how many decoded bytes, and what they are.
pub(super) type Run = (usize, Option<Highlight>);

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

    /// How many lines it holds, toward a review's budget.
    pub(super) fn lines(&self) -> usize {
        self.lines.len()
    }

    /// Whether every one of `numbers` was projected.
    pub(super) fn holds_all(&self, mut numbers: impl Iterator<Item = u32>) -> bool {
        numbers.all(|number| self.line(number).is_some())
    }
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
        let from = projected.runs.len();
        projected.lines.push((number, from));
        if bom && row == 0 {
            push(&mut projected.runs, from, BOM.len(), None);
        }
        let span = text.line_start(row)..text.line_end(row);
        for (run, highlight) in highlights.runs(span.clone()) {
            let run = run.start.max(span.start)..run.end.min(span.end);
            push(&mut projected.runs, from, run.len(), highlight);
        }
    }
    projected
}

/// Adds a run to a line's, whose first is at `from`: never merged with the line's before.
fn push(
    runs: &mut Vec<(u32, Option<Highlight>)>,
    from: usize,
    len: usize,
    highlight: Option<Highlight>,
) {
    let Ok(len) = u32::try_from(len) else {
        return;
    };
    if len == 0 {
        return;
    }
    let ours = runs.len() > from;
    match runs.last_mut() {
        Some((last, kept)) if ours && *kept == highlight => *last += len,
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

/// Walks a section's own runs, copying or skipping them a length at a time.
struct Base<'a> {
    runs: &'a [Run],
    at: usize,
    /// How far into `runs[at]` it is.
    into: usize,
}

impl Base<'_> {
    /// Copies the next `len` bytes' runs to `out`, or skips them.
    fn take(&mut self, mut len: usize, mut out: Option<&mut Vec<Run>>) {
        while len > 0 {
            let Some((run_len, highlight)) = self.runs.get(self.at).copied() else {
                return;
            };
            let step = (run_len - self.into).min(len);
            if let Some(out) = out.as_deref_mut() {
                merge(out, step, highlight);
            }
            len -= step;
            self.into += step;
            if self.into == run_len {
                self.at += 1;
                self.into = 0;
            }
        }
    }

    /// Copies what is left.
    fn rest(&mut self, out: &mut Vec<Run>) {
        self.take(usize::MAX, Some(out));
    }
}

fn merge(out: &mut Vec<Run>, len: usize, highlight: Option<Highlight>) {
    if len == 0 {
        return;
    }
    match out.last_mut() {
        Some((last, kept)) if *kept == highlight => *last += len,
        _ => out.push((len, highlight)),
    }
}

/// `base`, a section's own runs, with `sides`' runs, old then new, over the text of `slots`,
/// in one pass: a slot whose side has no runs for its line keeps the section's.
pub(super) fn merged(base: &[Run], slots: &[Slot], sides: [Option<&Projected>; 2]) -> Vec<Run> {
    let mut out = Vec::with_capacity(base.len());
    let mut walk = Base {
        runs: base,
        at: 0,
        into: 0,
    };
    let mut done = 0;
    for slot in slots {
        // Slots come in order and never meet; one that does not is left as the section has it.
        if slot.at < done {
            continue;
        }
        walk.take(slot.at - done, Some(&mut out));
        let side = match slot.side {
            Side::Old => sides[0],
            Side::New => sides[1],
        };
        match side.and_then(|projected| Some((projected, projected.line(slot.number)?))) {
            Some((projected, runs)) => {
                let plain = plain(slot.side, projected.done);
                let mut left = slot.len;
                for (len, highlight) in runs {
                    let len = (*len as usize).min(left);
                    merge(&mut out, len, highlight.or(plain));
                    left -= len;
                }
                // A line its side's text does not fill ends plain as well.
                merge(&mut out, left, plain);
                walk.take(slot.len, None);
            }
            None => walk.take(slot.len, Some(&mut out)),
        }
        done = slot.at + slot.len;
    }
    walk.rest(&mut out);
    out
}

/// `rendered`'s runs with `sides`' merged in, or none when it shows no line of a side or they
/// have no runs, and it keeps its own: as a text is laid out, the unified one or a pane.
pub(super) fn section_runs(
    rendered: &Rendered,
    sides: [Option<&Projected>; 2],
) -> Option<Vec<Run>> {
    let some = !rendered.slots.is_empty() && sides.iter().any(Option::is_some);
    some.then(|| merged(&rendered.runs, &rendered.slots, sides))
}

/// Lays `sides`' runs over `rendered`, a section that starts at `start` in `highlights`: one
/// replace over its span.
pub(super) fn paint(
    highlights: &mut Highlights,
    start: usize,
    rendered: &Rendered,
    sides: [Option<&Projected>; 2],
) {
    let runs = merged(&rendered.runs, &rendered.slots, sides);
    let end = start.saturating_add(rendered.decoded);
    highlights.replace(start..end, runs);
}
