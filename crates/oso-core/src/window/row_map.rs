//! How the lines of one window of a pair line up with the other's (ADR 0122, step 10).
//!
//! Must not depend on: `editor`, `command`, `frame` or anything that draws; a map is lines and
//! numbers, which its owner builds and the editor reads.
//!
//! A pair's first window is the left or the upper one, and its second the other. The map is
//! a list of aligned runs: a run of lines of the first that are, line by line, a run of lines
//! of the second. Lines between two runs are a gap on each side, and a gap's lines go to the
//! other side's gap line by line, its last line taking those left over, or to the line after
//! the run before it when the other side's gap is empty: the nearest line the other side has.
//! The lines are buffer lines, display rows before any wrap (`DisplayPoint::row`, a logical line
//! while no fold or inlay changes rows), so each window wraps at its own width and the map stays
//! true.
//!
//! Building a map checks every run, so an owner builds it where it builds the text it maps, off
//! the UI thread for a large one, and the editor only takes it over.

use std::fmt;

/// How the lines of a pair's first window line up with its second's: every line with the same
/// line ([`RowMap::same`]), or runs of aligned lines ([`RowMap::new`]). A lookup halves the
/// runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowMap {
    /// `None` for the same lines; otherwise the runs, as `(first's line, second's line,
    /// lines)`, each at least a line, in order and apart on both sides.
    runs: Option<Vec<(usize, usize, usize)>>,
}

/// A row map refused: a run of no lines, runs out of order or overlapping on either side, or
/// a run that ends past the last line a number holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BadRowMap;

impl fmt::Display for BadRowMap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the aligned runs are empty, out of order, overlapping or too long")
    }
}

impl std::error::Error for BadRowMap {}

impl RowMap {
    /// Each line of the first window is the same line of the second, as two sides that filler
    /// rows already align have it.
    pub fn same() -> Self {
        Self { runs: None }
    }

    /// The map of `runs`, each `(first's line, second's line, lines)`: those lines of the first
    /// window are those of the second, line by line.
    ///
    /// # Errors
    ///
    /// [`BadRowMap`] for a run of no lines, runs that are not in order and apart on both sides,
    /// or a run that ends past the last number.
    pub fn new(runs: Vec<(usize, usize, usize)>) -> Result<Self, BadRowMap> {
        let mut ends = (0, 0);
        for &(first, second, lines) in &runs {
            let first_end = first.checked_add(lines).ok_or(BadRowMap)?;
            let second_end = second.checked_add(lines).ok_or(BadRowMap)?;
            if lines == 0 || first < ends.0 || second < ends.1 {
                return Err(BadRowMap);
            }
            ends = (first_end, second_end);
        }
        Ok(Self { runs: Some(runs) })
    }

    /// The line of the second window that line `line` of the first goes to.
    pub fn second_of(&self, line: usize) -> usize {
        self.runs
            .as_deref()
            .map_or(line, |runs| across(runs, line, |(a, b, n)| (a, b, n)))
    }

    /// The line of the first window that line `line` of the second goes to.
    pub fn first_of(&self, line: usize) -> usize {
        self.runs
            .as_deref()
            .map_or(line, |runs| across(runs, line, |(a, b, n)| (b, a, n)))
    }
}

/// The line on the other side of `line`, with `side` giving each run as `(this side's line,
/// the other's, lines)`.
fn across(
    runs: &[(usize, usize, usize)],
    line: usize,
    side: impl Fn((usize, usize, usize)) -> (usize, usize, usize),
) -> usize {
    // The runs that start at or before `line`.
    let at = runs.partition_point(|&run| side(run).0 <= line);
    // The gap `line` is in, or the run before it, starts where the run before it ends.
    let (here, there) = match at.checked_sub(1).map(|i| side(runs[i])) {
        Some((here, there, lines)) if line < here + lines => return there + (line - here),
        Some((here, there, lines)) => (here + lines, there + lines),
        None => (0, 0),
    };
    let into = line - here;
    match runs.get(at).map(|&run| side(run)) {
        // A gap between runs, or before the first: line by line into the other side's gap,
        // its last line taking the rest, or the line after the run before it.
        Some((_, next_there, _)) => {
            let gap = next_there - there;
            there + into.min(gap.saturating_sub(1))
        }
        // Past the last run, both sides go on line by line.
        None => there.saturating_add(into),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn the_same_lines_go_to_themselves() {
        let map = RowMap::same();
        assert_eq!((map.second_of(0), map.first_of(41)), (0, 41));
    }

    #[test]
    fn a_run_maps_line_by_line_and_a_gap_goes_to_the_other_gap() {
        // Lines 0–2 are 0–2; the first has 3–6 alone, the second 3–4; 7–9 are 5–7.
        let map = RowMap::new(vec![(0, 0, 3), (7, 5, 3)]).unwrap();
        assert_eq!(map.second_of(1), 1);
        assert_eq!(map.second_of(8), 6);
        // The first's gap of four goes into the second's of two, its last line taking the rest.
        let gap: Vec<usize> = (3..7).map(|line| map.second_of(line)).collect();
        assert_eq!(gap, [3, 4, 4, 4]);
        assert_eq!(map.first_of(4), 4);
        // Past the last run, line by line.
        assert_eq!((map.second_of(12), map.first_of(10)), (10, 12));
    }

    #[test]
    fn a_gap_the_other_side_lacks_goes_to_the_line_after_the_run_before_it() {
        // The first has 2–4 that the second lacks: they go to the second's line 2.
        let map = RowMap::new(vec![(0, 0, 2), (5, 2, 4)]).unwrap();
        assert_eq!([2, 3, 4].map(|line| map.second_of(line)), [2, 2, 2]);
        // Before the first run, from line 0.
        let late = RowMap::new(vec![(4, 1, 2)]).unwrap();
        assert_eq!([0, 1, 2, 3].map(|line| late.second_of(line)), [0, 0, 0, 0]);
        assert_eq!(late.first_of(0), 0);
    }

    #[test]
    fn runs_out_of_order_overlapping_or_empty_are_refused() {
        assert_eq!(RowMap::new(vec![(0, 0, 0)]), Err(BadRowMap));
        assert_eq!(RowMap::new(vec![(0, 0, 3), (2, 5, 1)]), Err(BadRowMap));
        assert_eq!(RowMap::new(vec![(0, 4, 3), (5, 2, 1)]), Err(BadRowMap));
        assert_eq!(RowMap::new(vec![(usize::MAX, 0, 2)]), Err(BadRowMap));
        assert!(
            RowMap::new(vec![(0, 0, 3), (3, 3, 1)]).is_ok(),
            "runs may touch"
        );
        assert!(RowMap::new(Vec::new()).is_ok());
    }

    /// Runs in order and apart on both sides, from gaps and lengths.
    fn runs() -> impl Strategy<Value = Vec<(usize, usize, usize)>> {
        prop::collection::vec((0..5usize, 0..5usize, 1..6usize), 0..12).prop_map(|parts| {
            let (mut a, mut b) = (0, 0);
            parts
                .into_iter()
                .map(|(gap_a, gap_b, lines)| {
                    let run = (a + gap_a, b + gap_b, lines);
                    (a, b) = (run.0 + lines, run.1 + lines);
                    run
                })
                .collect()
        })
    }

    proptest! {
        #[test]
        fn runs_map_exactly_and_back(runs in runs()) {
            let map = RowMap::new(runs.clone()).unwrap();
            for (a, b, lines) in runs {
                for i in 0..lines {
                    prop_assert_eq!(map.second_of(a + i), b + i);
                    prop_assert_eq!(map.first_of(b + i), a + i);
                }
            }
        }

        #[test]
        fn the_map_keeps_the_lines_in_order(runs in runs(), line in 0..80usize) {
            let map = RowMap::new(runs).unwrap();
            prop_assert!(map.second_of(line) <= map.second_of(line + 1));
            prop_assert!(map.first_of(line) <= map.first_of(line + 1));
        }
    }
}
