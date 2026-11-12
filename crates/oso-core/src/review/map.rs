//! Where a line of one reading of a file is in the next (ADR 0122, "Comments and
//! re-anchoring"), from the two readings' shapes alone.
//!
//! Must not depend on: `editor`, `buffer` or anything outside the review, or I/O. The longest
//! common subsequence is the caller's, handed in as a function, and what it returns is checked
//! here.
//!
//! The diff has no context, so the text around a line says little about where it went: a blank
//! line or a lone brace reads like a hundred others. So nothing here guesses from nearby text.
//! The new side of a reading is a sequence in which a line the old side had is its number there
//! (an identity, not a text) and a line put in is its text. Two readings against the same old
//! side share every old line neither took out, and those match by number; only the lines put in
//! between two shared lines are matched by text, and only with each other. When the old side
//! changed, its numbers are carried through the hunks between the two old sides first; when the
//! old sides are not known, lines match by text and order alone; when nothing can be said,
//! nothing matches.
//!
//! The work follows the hunks, not the length of the file: the shared lines are found as runs
//! between the hunks' edges, and only the lines put in are handed to the matcher.

use std::ops::Range;
use std::sync::Arc;

use super::shape::{Base, Shape, Side};

/// A hunk of one old side against another, by line numbers counted from 1: `old` of the first
/// became `new` of the second. An empty `old` goes in front of its start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BaseHunk {
    pub old: Range<u32>,
    pub new: Range<u32>,
}

/// The two blobs whose hunks [`LineMap::new`] needs to carry `from` to `to`: `Some` only when
/// both readings are against blobs, and different ones.
pub fn base_pair<'a>(from: &'a Shape, to: &'a Shape) -> Option<(&'a [u8], &'a [u8])> {
    match (from.base(), to.base()) {
        (Base::Blob(a), Base::Blob(b)) if a != b => Some((a, b)),
        _ => None,
    }
}

/// Where each line of one reading of a file is in another.
#[derive(Clone, Debug)]
pub struct LineMap {
    kind: Kind,
}

#[derive(Clone, Debug)]
enum Kind {
    /// Nothing can be said: no line is carried.
    Nothing,
    /// The old sides are not known: lines put in and lines taken out, each matched by text.
    ByText {
        new: Vec<(u32, u32)>,
        old: Vec<(u32, u32)>,
    },
    /// Old lines carried by number through `through`; lines put in matched between them.
    Shared {
        from: Spans,
        to: Arc<Spans>,
        through: Through,
        put: Vec<(u32, u32)>,
    },
}

impl LineMap {
    /// The map from the reading `from` to the reading `to`. `base` is the hunks of `from`'s old
    /// blob against `to`'s, which only two different blobs need ([`base_pair`]); `None` for
    /// those means they could not be compared, and nothing is carried. `lcs` is given two runs
    /// of lines and returns the pairs of indices it matches, increasing in both; a run whose
    /// pairs are out of range, out of order or of unequal lines has none of them kept.
    ///
    /// `lcs` must bound its own work, as a diff with a deadline does: it may be handed every
    /// line both readings put in.
    pub fn new<F>(from: &Shape, to: &Shape, base: Option<&[BaseHunk]>, lcs: F) -> LineMap
    where
        F: FnMut(&[&[u8]], &[&[u8]]) -> Vec<(usize, usize)>,
    {
        LineMap::onto(from, to, &Arc::new(Spans::of(to)), base, lcs)
    }

    /// [`LineMap::new`] with `to`'s edges taken once, for maps from many readings to one.
    pub(super) fn onto<F>(
        from: &Shape,
        to: &Shape,
        to_spans: &Arc<Spans>,
        base: Option<&[BaseHunk]>,
        mut lcs: F,
    ) -> LineMap
    where
        F: FnMut(&[&[u8]], &[&[u8]]) -> Vec<(usize, usize)>,
    {
        let through = match (from.base(), to.base()) {
            (Base::Unknown, _) | (_, Base::Unknown) => {
                let new = by_text(from, to, Side::New, &mut lcs);
                let old = by_text(from, to, Side::Old, &mut lcs);
                return LineMap {
                    kind: Kind::ByText { new, old },
                };
            }
            (a, b) if a == b => Through::Same,
            (Base::Empty, _) | (_, Base::Empty) => Through::Nothing,
            _ => match base.and_then(Shifter::pair) {
                Some((forward, backward)) => Through::Hunks { forward, backward },
                None => {
                    return LineMap {
                        kind: Kind::Nothing,
                    };
                }
            },
        };
        let from_spans = Spans::of(from);
        let put = match_put_in(from, to, &from_spans, to_spans, &through, &mut lcs);
        LineMap {
            kind: Kind::Shared {
                from: from_spans,
                to: Arc::clone(to_spans),
                through,
                put,
            },
        }
    }

    /// The number that line `number` of `side` of the first reading has on that side of the
    /// second, or `None` when the second does not have it: it changed, or it went. A number
    /// returned may be of a line the second reading no longer shows, which the caller checks.
    pub fn carry(&self, side: Side, number: u32) -> Option<u32> {
        match &self.kind {
            Kind::Nothing => None,
            Kind::ByText { new, old } => {
                let pairs = if side == Side::New { new } else { old };
                look_up(pairs, number)
            }
            Kind::Shared {
                from,
                to,
                through,
                put,
            } => match side {
                Side::Old => through.forward(number),
                Side::New => match from.new_to_old(number) {
                    Kept::Old(old) => to.old_to_new(through.forward(old)?),
                    Kept::Put => look_up(put, number),
                },
            },
        }
    }
}

/// The second number of the pair whose first is `number`, in `pairs` sorted by their first.
fn look_up(pairs: &[(u32, u32)], number: u32) -> Option<u32> {
    let at = pairs.binary_search_by_key(&number, |pair| pair.0).ok()?;
    Some(pairs[at].1)
}

/// How the old side of one reading became that of the other.
#[derive(Clone, Debug)]
enum Through {
    Same,
    /// No line of the one is in the other: a file put in, or one that went.
    Nothing,
    Hunks {
        forward: Shifter,
        backward: Shifter,
    },
}

impl Through {
    fn forward(&self, number: u32) -> Option<u32> {
        match self {
            Through::Same => Some(number),
            Through::Nothing => None,
            Through::Hunks { forward, .. } => forward.shift(number),
        }
    }

    fn backward(&self, number: u32) -> Option<u32> {
        match self {
            Through::Same => Some(number),
            Through::Nothing => None,
            Through::Hunks { backward, .. } => backward.shift(number),
        }
    }

    /// The first old lines of the first reading at which carrying a line may start or stop
    /// working: the edges of the hunks between the two old sides.
    fn edges(&self, out: &mut Vec<u32>) {
        if let Through::Hunks { forward, .. } = self {
            out.extend(forward.starts.iter().chain(&forward.ends));
        }
    }
}

/// Carries a line number through hunks that each replace `start..end` with lines that shift
/// what follows them. Lines are found among the hunks by halving.
#[derive(Clone, Debug)]
struct Shifter {
    starts: Vec<u32>,
    ends: Vec<u32>,
    /// `shifts[k]`: how far the hunks before hunk `k` move a line.
    shifts: Vec<i64>,
}

impl Shifter {
    /// The shifters forward and backward through `hunks`, or `None` when they are not in
    /// order or not numbered consistently.
    fn pair(hunks: &[BaseHunk]) -> Option<(Shifter, Shifter)> {
        let mut shift: i64 = 0;
        let mut previous: Option<&BaseHunk> = None;
        for hunk in hunks {
            let ordered = hunk.old.start >= 1
                && hunk.old.start <= hunk.old.end
                && hunk.new.start <= hunk.new.end
                && previous.is_none_or(|before| {
                    hunk.old.start >= before.old.end && hunk.new.start >= before.new.end
                });
            if !ordered || i64::from(hunk.new.start) != i64::from(hunk.old.start) + shift {
                return None;
            }
            shift +=
                i64::from(hunk.new.end - hunk.new.start) - i64::from(hunk.old.end - hunk.old.start);
            previous = Some(hunk);
        }
        let forward = Shifter::new(hunks.iter().map(|hunk| (&hunk.old, &hunk.new)));
        let backward = Shifter::new(hunks.iter().map(|hunk| (&hunk.new, &hunk.old)));
        Some((forward, backward))
    }

    fn new<'a>(hunks: impl Iterator<Item = (&'a Range<u32>, &'a Range<u32>)>) -> Shifter {
        let mut shifter = Shifter {
            starts: Vec::new(),
            ends: Vec::new(),
            shifts: vec![0],
        };
        for (here, there) in hunks {
            shifter.starts.push(here.start);
            shifter.ends.push(here.end);
            let last = shifter.shifts.last().copied().unwrap_or(0);
            shifter
                .shifts
                .push(last + i64::from(there.end - there.start) - i64::from(here.end - here.start));
        }
        shifter
    }

    /// Where line `number` goes, or `None` for a line a hunk replaced.
    fn shift(&self, number: u32) -> Option<u32> {
        let above = self.ends.partition_point(|&end| end <= number);
        if self.starts.get(above).is_some_and(|&start| start <= number) {
            return None;
        }
        u32::try_from(i64::from(number) + self.shifts[above]).ok()
    }
}

/// The edges of a reading's hunks, without their text, to carry numbers by.
#[derive(Clone, Debug)]
pub(super) struct Spans {
    spans: Vec<Span>,
}

#[derive(Clone, Copy, Debug)]
struct Span {
    old: u32,
    removed: u32,
    new: u32,
    added: u32,
}

impl Span {
    /// How far the hunks up to and including this one move the old lines after it.
    fn shift_after(self) -> i64 {
        i64::from(self.new) + i64::from(self.added) - i64::from(self.old) - i64::from(self.removed)
    }
}

/// What a line of the new side is.
enum Kept {
    /// Kept from the old side, where it is this line.
    Old(u32),
    /// Put in by the diff.
    Put,
}

impl Spans {
    pub(super) fn of(shape: &Shape) -> Spans {
        let spans = shape
            .hunks()
            .iter()
            .map(|hunk| Span {
                old: hunk.old_start,
                removed: count(hunk.removed.len()),
                new: hunk.new_start,
                added: count(hunk.added.len()),
            })
            .collect();
        Spans { spans }
    }

    fn new_to_old(&self, number: u32) -> Kept {
        let after = self.spans.partition_point(|span| span.new <= number);
        let Some(span) = after.checked_sub(1).map(|index| self.spans[index]) else {
            return Kept::Old(number);
        };
        if number - span.new < span.added {
            return Kept::Put;
        }
        // A number past the last hunk keeps its offset, so it never leaves 32 bits backward.
        u32::try_from(i64::from(number) - span.shift_after()).map_or(Kept::Put, Kept::Old)
    }

    fn old_to_new(&self, number: u32) -> Option<u32> {
        let after = self.spans.partition_point(|span| span.old <= number);
        let Some(span) = after.checked_sub(1).map(|index| self.spans[index]) else {
            return (number >= 1).then_some(number);
        };
        if number - span.old < span.removed {
            return None;
        }
        u32::try_from(i64::from(number) + span.shift_after()).ok()
    }
}

/// A count of lines, which [`Shape::new`] has checked fits in 32 bits.
fn count(lines: usize) -> u32 {
    u32::try_from(lines).unwrap_or(u32::MAX)
}

/// The lines put in by `from` matched with those put in by `to`, as pairs of new-side numbers
/// sorted by the first: only lines between the same two shared old lines are compared.
fn match_put_in<F>(
    from: &Shape,
    to: &Shape,
    from_spans: &Spans,
    to_spans: &Spans,
    through: &Through,
    lcs: &mut F,
) -> Vec<(u32, u32)>
where
    F: FnMut(&[&[u8]], &[&[u8]]) -> Vec<(usize, usize)>,
{
    let mut put = Vec::new();
    // The last shared line before the stretch being matched, on each side; 0 before the first.
    let mut after = (0, 0);
    for run in shared_runs(from_spans, to_spans, through) {
        let first = (run.from_new, run.to_new);
        match_stretch(from, to, after, Some(first), lcs, &mut put);
        let Some(len) = run.len else {
            return put;
        };
        after = (
            run.from_new.saturating_add(len - 1),
            run.to_new.saturating_add(len - 1),
        );
    }
    match_stretch(from, to, after, None, lcs, &mut put);
    put
}

/// A run of old lines both readings keep, at these first new-side numbers, `len` lines long,
/// or to the end of the file for `None`.
struct Run {
    from_new: u32,
    to_new: u32,
    len: Option<u32>,
}

/// The runs of old lines of the first reading that both readings keep, in order. Whether a
/// line is shared changes only at an edge of a hunk of either reading or between the old
/// sides, so each stretch between two edges is shared whole or not at all.
fn shared_runs(from: &Spans, to: &Spans, through: &Through) -> Vec<Run> {
    if matches!(through, Through::Nothing) {
        return Vec::new();
    }
    let mut edges = vec![1];
    for span in &from.spans {
        edges.extend([span.old, span.old.saturating_add(span.removed)]);
    }
    through.edges(&mut edges);
    for span in &to.spans {
        for edge in [span.old, span.old.saturating_add(span.removed)] {
            edges.extend(through.backward(edge));
        }
    }
    edges.sort_unstable();
    edges.dedup();
    let mut runs = Vec::new();
    for (index, &start) in edges.iter().enumerate() {
        let shared = from.old_to_new(start).and_then(|from_new| {
            let to_new = to.old_to_new(through.forward(start)?)?;
            Some((from_new, to_new))
        });
        if let Some((from_new, to_new)) = shared {
            let len = edges.get(index + 1).map(|&end| end - start);
            runs.push(Run {
                from_new,
                to_new,
                len,
            });
        }
    }
    runs
}

/// Matches the lines put in strictly between `after` and `before` (new-side numbers of the two
/// readings; `None` for the end of the file), pushing the pairs onto `put`.
fn match_stretch<F>(
    from: &Shape,
    to: &Shape,
    after: (u32, u32),
    before: Option<(u32, u32)>,
    lcs: &mut F,
    put: &mut Vec<(u32, u32)>,
) where
    F: FnMut(&[&[u8]], &[&[u8]]) -> Vec<(usize, usize)>,
{
    let (from_lines, from_numbers) = put_in_between(from, after.0, before.map(|b| b.0));
    let (to_lines, to_numbers) = put_in_between(to, after.1, before.map(|b| b.1));
    if from_lines.is_empty() || to_lines.is_empty() {
        return;
    }
    if let Some(pairs) = checked(&from_lines, &to_lines, lcs(&from_lines, &to_lines)) {
        put.extend(
            pairs
                .into_iter()
                .map(|(i, j)| (from_numbers[i], to_numbers[j])),
        );
    }
}

/// The lines `shape` puts in with new-side numbers strictly between `after` and `before`, and
/// their numbers.
fn put_in_between(shape: &Shape, after: u32, before: Option<u32>) -> (Vec<&[u8]>, Vec<u32>) {
    let before = before.unwrap_or(u32::MAX);
    let hunks = shape.hunks();
    let first = hunks.partition_point(|hunk| {
        u64::from(hunk.new_start) + hunk.added.len() as u64 <= u64::from(after) + 1
    });
    let mut lines = Vec::new();
    let mut numbers = Vec::new();
    for hunk in hunks[first..]
        .iter()
        .take_while(|hunk| hunk.new_start < before)
    {
        for (line, number) in hunk.added.iter().zip(hunk.new_start..) {
            if number > after && number < before {
                lines.push(&**line);
                numbers.push(number);
            }
        }
    }
    (lines, numbers)
}

/// The matched lines of `side` of both readings, by text and order alone, as pairs of numbers.
fn by_text<F>(from: &Shape, to: &Shape, side: Side, lcs: &mut F) -> Vec<(u32, u32)>
where
    F: FnMut(&[&[u8]], &[&[u8]]) -> Vec<(usize, usize)>,
{
    let (from_lines, from_numbers) = all_lines(from, side);
    let (to_lines, to_numbers) = all_lines(to, side);
    if from_lines.is_empty() || to_lines.is_empty() {
        return Vec::new();
    }
    checked(&from_lines, &to_lines, lcs(&from_lines, &to_lines))
        .unwrap_or_default()
        .into_iter()
        .map(|(i, j)| (from_numbers[i], to_numbers[j]))
        .collect()
}

/// Every line of `side` the diff shows, and their numbers.
fn all_lines(shape: &Shape, side: Side) -> (Vec<&[u8]>, Vec<u32>) {
    let mut lines = Vec::new();
    let mut numbers = Vec::new();
    for hunk in shape.hunks() {
        let (start, side_lines) = super::shape::side_of(hunk, side);
        for (line, number) in side_lines.iter().zip(start..) {
            lines.push(&**line);
            numbers.push(number);
        }
    }
    (lines, numbers)
}

/// `pairs` when they are in range, increasing in both and of equal lines; else `None`.
fn checked(a: &[&[u8]], b: &[&[u8]], pairs: Vec<(usize, usize)>) -> Option<Vec<(usize, usize)>> {
    let mut next = (0, 0);
    for &(i, j) in &pairs {
        let good =
            i >= next.0 && j >= next.1 && a.get(i).is_some_and(|line| b.get(j) == Some(line));
        if !good {
            return None;
        }
        next = (i + 1, j + 1);
    }
    Some(pairs)
}

#[cfg(test)]
mod tests;
