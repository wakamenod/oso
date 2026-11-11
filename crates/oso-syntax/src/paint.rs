//! Turns a piece's captures into runs of highlights (ADR 0111): an inner capture paints over
//! an outer one, and of two with the same span the first pattern wins. Line breaks stay plain.
//!
//! Must not depend on: `document` or `queries`, nor tree-sitter's types; it sees spans and
//! classes only.
//!
//! - One sweep over the captures sorted by where they start, with a stack of those still open:
//!   O(k log k) for k captures and O(1) a run, however deeply they nest.
//! - The piece's line breaks are read from the rope once, and passed in order.

use std::cmp::Reverse;
use std::ops::Range;

use oso_core::rope::Rope;
use oso_core::syntax::Highlight;

/// One captured span with its class.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Capture {
    pub(crate) range: Range<usize>,
    /// Which query found it: of two with the same span, a later layer (an injected grammar's)
    /// wins.
    pub(crate) layer: u8,
    /// Its pattern in that query: of two with the same span, the first wins.
    pub(crate) pattern: usize,
    pub(crate) class: Highlight,
}

/// The runs of `piece`, as lengths with what each is, from `captures`, which may reach past it.
pub(crate) fn paint(
    text: &Rope,
    piece: Range<usize>,
    mut captures: Vec<Capture>,
) -> Vec<(usize, Option<Highlight>)> {
    for capture in &mut captures {
        capture.range.start = capture.range.start.max(piece.start);
        capture.range.end = capture.range.end.min(piece.end);
    }
    captures.retain(|capture| capture.range.start < capture.range.end);
    captures.sort_unstable_by_key(|capture| {
        (
            capture.range.start,
            Reverse(capture.range.end),
            capture.layer,
            Reverse(capture.pattern),
        )
    });
    // The piece's line breaks, read once: the sweep only goes forward.
    let mut breaks = Vec::new();
    let mut chunk_start = piece.start;
    for chunk in text.chunks_in_range(piece.clone()) {
        breaks.extend(
            chunk
                .match_indices('\n')
                .map(|(index, _)| chunk_start + index),
        );
        chunk_start += chunk.len();
    }
    let mut sweep = Sweep {
        breaks,
        next_break: 0,
        at: piece.start,
        open: Vec::new(),
        runs: Vec::new(),
    };
    for capture in captures {
        sweep.advance(capture.range.start);
        sweep.open.push((capture.range.end, capture.class));
    }
    sweep.advance(piece.end);
    sweep.runs
}

struct Sweep {
    /// The piece's line breaks, ascending, and the first not yet passed.
    breaks: Vec<usize>,
    next_break: usize,
    at: usize,
    /// The captures open at `at`, the innermost last, each as its end and its class.
    open: Vec<(usize, Highlight)>,
    runs: Vec<(usize, Option<Highlight>)>,
}

impl Sweep {
    /// Paints from `at` to `to` with the innermost open capture, closing those that end.
    fn advance(&mut self, to: usize) {
        while self.at < to {
            while self.open.last().is_some_and(|(end, _)| *end <= self.at) {
                self.open.pop();
            }
            let (end, class) = match self.open.last() {
                Some(&(end, class)) => (end.min(to), Some(class)),
                None => (to, None),
            };
            match class {
                Some(class) => self.highlighted(self.at..end, class),
                None => push(&mut self.runs, end - self.at, None),
            }
            self.at = end;
        }
    }

    /// Paints `range` with `class`, leaving its line breaks plain.
    fn highlighted(&mut self, range: Range<usize>, class: Highlight) {
        let mut painted = range.start;
        while let Some(&line_break) = self.breaks.get(self.next_break) {
            if line_break >= range.end {
                break;
            }
            self.next_break += 1;
            if line_break < painted {
                continue;
            }
            push(&mut self.runs, line_break - painted, Some(class));
            push(&mut self.runs, 1, None);
            painted = line_break + 1;
        }
        push(&mut self.runs, range.end - painted, Some(class));
    }
}

/// Adds a run, joining the last one if it has the same highlight.
fn push(runs: &mut Vec<(usize, Option<Highlight>)>, len: usize, class: Option<Highlight>) {
    if len == 0 {
        return;
    }
    match runs.last_mut() {
        Some((last, highlight)) if *highlight == class => *last += len,
        _ => runs.push((len, class)),
    }
}

#[cfg(test)]
mod tests;
