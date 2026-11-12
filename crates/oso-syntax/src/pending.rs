//! The ranges of a document's text still to highlight, mapped through its edits (ADR 0111).
//!
//! Must not depend on: `document` or `queries`, nor tree-sitter's types; it sees offsets only.
//!
//! - The ranges are ascending, apart and never empty. Ranges that touch join, unless the set
//!   keeps them apart ([`Pending::apart`]), as it does for fences that follow each other.
//! - Mapping through a batch and adding a batch's ranges each take one pass, so a command at a
//!   thousand cursors costs no more than its splices; taking a range out is a binary search.

use std::ops::Range;

use oso_core::syntax::Splice;

/// Ranges still to highlight, or another set of ranges that follows edits.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Pending {
    ranges: Vec<Range<usize>>,
    /// Whether ranges that only touch stay apart.
    apart: bool,
}

impl Pending {
    /// No ranges, and ranges added or mapped later that only touch stay apart.
    pub(crate) fn apart() -> Self {
        Self {
            ranges: Vec::new(),
            apart: true,
        }
    }

    /// Whether nothing is left.
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    /// The ranges, ascending.
    pub(crate) fn ranges(&self) -> &[Range<usize>] {
        &self.ranges
    }

    /// Nothing left.
    pub(crate) fn clear(&mut self) {
        self.ranges.clear();
    }

    /// Adds `ranges`, ascending, which may meet each other and those there.
    pub(crate) fn add(&mut self, ranges: impl IntoIterator<Item = Range<usize>>) {
        let old = std::mem::take(&mut self.ranges);
        let mut old = old.into_iter().peekable();
        let mut new = ranges
            .into_iter()
            .filter(|range| range.start < range.end)
            .peekable();
        loop {
            let next = match (old.peek(), new.peek()) {
                (Some(a), Some(b)) if a.start <= b.start => old.next(),
                (Some(_), Some(_)) | (None, Some(_)) => new.next(),
                (Some(_), None) => old.next(),
                (None, None) => break,
            };
            if let Some(range) = next {
                self.push(range);
            }
        }
    }

    /// Makes all of a text of `len` bytes pending.
    pub(crate) fn all(&mut self, len: usize) {
        self.clear();
        self.add(std::iter::once(0..len));
    }

    /// Takes `range` out, in place: O(log n) to find the ranges it meets, and a move of those
    /// after them.
    pub(crate) fn remove(&mut self, range: Range<usize>) {
        if range.start >= range.end {
            return;
        }
        let first = self
            .ranges
            .partition_point(|pending| pending.end <= range.start);
        let last = self
            .ranges
            .partition_point(|pending| pending.start < range.end);
        if first >= last {
            return;
        }
        let (head, tail) = (self.ranges[first].start, self.ranges[last - 1].end);
        let kept = [head..range.start, range.end..tail];
        self.ranges.splice(
            first..last,
            kept.into_iter().filter(|kept| kept.start < kept.end),
        );
    }

    /// Puts `ranges`, ascending, apart and inside `within`, in place of the ranges that meet
    /// `within`, and gives `within` widened to those it replaced: a binary search, and a move of
    /// the ranges after them.
    pub(crate) fn swap(&mut self, within: Range<usize>, ranges: &[Range<usize>]) -> Range<usize> {
        let first = self
            .ranges
            .partition_point(|range| range.end <= within.start);
        let last = self
            .ranges
            .partition_point(|range| range.start < within.end);
        let mut span = within;
        if first < last {
            span.start = span.start.min(self.ranges[first].start);
            span.end = span.end.max(self.ranges[last - 1].end);
        }
        let new = ranges
            .iter()
            .filter(|range| range.start < range.end)
            .cloned();
        self.ranges.splice(first..last.max(first), new);
        span
    }

    /// Follows a batch of edits, as [`Splice`] says. A range that a splice cuts keeps the text
    /// around it, and one the splice took in whole goes: the splice's own range is added apart.
    pub(crate) fn map(&mut self, splices: &[Splice]) {
        if splices.is_empty() || self.ranges.is_empty() {
            return;
        }
        let old = std::mem::take(&mut self.ranges);
        let (mut starts, mut ends) = (Mapper::new(splices), Mapper::new(splices));
        for range in old {
            let start = starts.start(range.start);
            let end = ends.end(range.end);
            if start < end {
                self.push(start..end);
            }
        }
    }

    /// Appends `range`, which starts at or after the last one's start, joining it if they
    /// overlap, or touch where ranges join.
    fn push(&mut self, range: Range<usize>) {
        let apart = self.apart;
        match self.ranges.last_mut() {
            Some(last) if range.start < last.end || (!apart && range.start == last.end) => {
                last.end = last.end.max(range.end);
            }
            _ => self.ranges.push(range),
        }
    }
}

/// Maps ascending offsets through a batch, one pass for them all.
struct Mapper<'a> {
    splices: &'a [Splice],
    /// The splices passed so far, and how much they moved what follows them.
    passed: usize,
    delta: isize,
}

impl<'a> Mapper<'a> {
    fn new(splices: &'a [Splice]) -> Self {
        Self {
            splices,
            passed: 0,
            delta: 0,
        }
    }

    /// Passes the splices for which `passes` holds, and gives the one that holds `offset`
    /// inside it, if any.
    fn pass(&mut self, offset: usize, passes: impl Fn(&Splice) -> bool) -> Option<&'a Splice> {
        while let Some(splice) = self
            .splices
            .get(self.passed)
            .filter(|splice| passes(splice))
        {
            self.delta += splice.new_len as isize - splice.old_len as isize;
            self.passed += 1;
        }
        self.splices
            .get(self.passed)
            .filter(|splice| splice.start < offset && offset < splice.start + splice.old_len)
    }

    fn shifted(&self, offset: usize) -> usize {
        offset.saturating_add_signed(self.delta)
    }

    /// Where a range starting at `offset` starts after the batch: past a splice that holds it.
    fn start(&mut self, offset: usize) -> usize {
        // Text inserted at the start is the splice's own, not the range's.
        match self.pass(offset, |splice| splice.start + splice.old_len <= offset) {
            Some(splice) => self.shifted(splice.start) + splice.new_len,
            None => self.shifted(offset),
        }
    }

    /// Where a range ending at `offset` ends after the batch: before a splice that holds it.
    fn end(&mut self, offset: usize) -> usize {
        // Text inserted at the end is the splice's own, not the range's.
        let passes =
            |splice: &Splice| splice.start < offset && splice.start + splice.old_len <= offset;
        match self.pass(offset, passes) {
            Some(splice) => self.shifted(splice.start),
            None => self.shifted(offset),
        }
    }
}

#[cfg(test)]
mod tests;
