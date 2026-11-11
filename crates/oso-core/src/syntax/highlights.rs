//! A buffer's highlights: runs of text, each with what it is or nothing, on a [`SumTree`]
//! (ADR 0111). The worker makes them; the buffer maps them through its edits until the next
//! report, and a frame reads the runs of the rows it draws.
//!
//! Must not depend on: `compose`; it sees only offsets.
//!
//! - Neighbouring runs never have the same highlight, and no run is empty.
//! - The worker leaves line breaks unhighlighted, so text inserted at a line's start, whose
//!   byte before is a line break, guesses nothing.
//! - Nothing here panics: a range past the end is cut to it.

use std::ops::Range;

use super::{Highlight, Splice};
use crate::sum_tree::{Bias, Cursor, Dimension, Item, SumTree, Summary};

/// One run: `len` bytes with one highlight, or none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Run {
    len: usize,
    highlight: Option<Highlight>,
}

/// The bytes of a run of runs.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Bytes(usize);

impl Summary for Bytes {
    fn add_summary(&mut self, other: &Self) {
        self.0 += other.0;
    }
}

impl Item for Run {
    type Summary = Bytes;

    fn summary(&self) -> Bytes {
        Bytes(self.len)
    }
}

impl Dimension<Bytes> for usize {
    fn add_summary(&mut self, summary: &Bytes) {
        *self += summary.0;
    }
}

/// What a buffer's text is, run by run. O(1) to clone, and it can go to another thread.
#[derive(Clone, Debug, Default)]
pub struct Highlights {
    runs: SumTree<Run>,
}

impl Highlights {
    /// `len` bytes with no highlight.
    pub fn new(len: usize) -> Self {
        let mut highlights = Self::default();
        if len > 0 {
            highlights.runs.push(Run {
                len,
                highlight: None,
            });
        }
        highlights
    }

    /// How many bytes it covers, which is its buffer's length.
    pub fn len(&self) -> usize {
        self.runs.extent::<usize>()
    }

    /// Whether it covers no text.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The runs that meet `range`, each cut to it, with what each is. O(log n) to the first, and
    /// O(1) each after that.
    pub fn runs(&self, range: Range<usize>) -> Runs<'_> {
        let end = range.end.min(self.len());
        let mut cursor = self.runs.cursor::<usize>();
        cursor.seek(&range.start, Bias::Right);
        Runs {
            cursor,
            at: range.start.min(end),
            end,
        }
    }

    /// What the byte at `offset` is, or `None` past the end.
    pub fn at(&self, offset: usize) -> Option<Highlight> {
        self.runs(offset..offset.saturating_add(1))
            .next()
            .and_then(|(_, highlight)| highlight)
    }

    /// Puts `runs`, given as lengths with what each is, in place of `range`. Runs past the
    /// range's length are cut, and a range they leave short ends with no highlight. O(log n),
    /// and O(1) a run given.
    pub fn replace(
        &mut self,
        range: Range<usize>,
        runs: impl IntoIterator<Item = (usize, Option<Highlight>)>,
    ) {
        let len = self.len();
        let (start, end) = (
            range.start.min(len),
            range.end.min(len).max(range.start.min(len)),
        );
        let mut middle = Vec::new();
        let mut left = end - start;
        for (run_len, highlight) in runs {
            let run_len = run_len.min(left);
            left -= run_len;
            push_run(&mut middle, run_len, highlight);
        }
        push_run(&mut middle, left, None);
        self.put(start..end, middle);
    }

    /// Follows a batch of edits, as [`Splice`] says: replaced text goes, and new text takes the
    /// highlight of the byte before it, as Emacs's text is rear-sticky. At the buffer's start,
    /// and after a line break, which the worker leaves plain, it takes none. The next report
    /// puts right what this guessed. O(log n) a splice.
    pub fn splice(&mut self, splices: &[Splice]) {
        for splice in splices.iter().rev() {
            let guess = splice
                .start
                .checked_sub(1)
                .and_then(|before| self.at(before));
            let end = splice.start.saturating_add(splice.old_len);
            let mut middle = Vec::new();
            push_run(&mut middle, splice.new_len, guess);
            let len = self.len();
            self.put(splice.start.min(len)..end.min(len), middle);
        }
    }

    /// Puts `middle`, runs with neighbours apart and none empty, in place of `range`, joining
    /// a run at either edge that has the same highlight as its neighbour.
    fn put(&mut self, range: Range<usize>, mut middle: Vec<Run>) {
        let (mut left, rest) = split(&self.runs, range.start);
        let (_, mut right) = split(&rest, range.end - range.start);
        let next = middle.first().copied().or_else(|| first_run(&right));
        if let Some(last) = last_run(&left)
            && next.is_some_and(|next| next.highlight == last.highlight)
        {
            left = split(&left, left.extent::<usize>() - last.len).0;
            middle.insert(0, last);
            coalesce(&mut middle);
        }
        if let Some(first) = first_run(&right)
            && middle
                .last()
                .is_some_and(|last| last.highlight == first.highlight)
        {
            right = split(&right, first.len).1;
            middle.push(first);
            coalesce(&mut middle);
        }
        left.append(middle.into_iter().collect());
        left.append(right);
        self.runs = left;
    }
}

/// Splits `tree` at byte `at`, cutting the run there in two.
fn split(tree: &SumTree<Run>, at: usize) -> (SumTree<Run>, SumTree<Run>) {
    let (mut left, right) = tree.split(&at, Bias::Right);
    let before = left.extent::<usize>();
    let Some(cut) = first_run(&right).filter(|_| before < at) else {
        return (left, right);
    };
    let (_, mut rest) = right.split(&cut.len, Bias::Right);
    let head = at - before;
    left.push(Run {
        len: head,
        highlight: cut.highlight,
    });
    let mut tail = SumTree::from_item(Run {
        len: cut.len - head,
        highlight: cut.highlight,
    });
    tail.append(std::mem::take(&mut rest));
    (left, tail)
}

fn first_run(tree: &SumTree<Run>) -> Option<Run> {
    tree.cursor::<usize>().item().copied()
}

fn last_run(tree: &SumTree<Run>) -> Option<Run> {
    let mut cursor = tree.cursor::<usize>();
    cursor.seek(&tree.extent::<usize>(), Bias::Left);
    cursor.item().copied()
}

/// Adds a run of `len` bytes, joining the last one if it has the same highlight.
fn push_run(runs: &mut Vec<Run>, len: usize, highlight: Option<Highlight>) {
    if len == 0 {
        return;
    }
    match runs.last_mut() {
        Some(last) if last.highlight == highlight => last.len += len,
        _ => runs.push(Run { len, highlight }),
    }
}

/// Joins neighbours with the same highlight.
fn coalesce(runs: &mut Vec<Run>) {
    let mut out = Vec::with_capacity(runs.len());
    for run in runs.drain(..) {
        push_run(&mut out, run.len, run.highlight);
    }
    *runs = out;
}

/// The runs of a range, from [`Highlights::runs`]: each byte range with what it is.
pub struct Runs<'a> {
    cursor: Cursor<'a, Run, usize>,
    at: usize,
    end: usize,
}

impl Iterator for Runs<'_> {
    type Item = (Range<usize>, Option<Highlight>);

    fn next(&mut self) -> Option<Self::Item> {
        if self.at >= self.end {
            return None;
        }
        let run = self.cursor.item()?;
        let run_end = self.cursor.end().min(self.end);
        let range = self.at..run_end;
        self.at = run_end;
        self.cursor.next();
        Some((range, run.highlight))
    }
}

#[cfg(test)]
mod tests;
