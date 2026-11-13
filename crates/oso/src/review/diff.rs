//! Diffing a review's sides in-process with `similar` (ADR 0122): Myers' diff with a deadline,
//! grouped into hunks with no context lines, as `oso_core::review::Shape` takes them. Also the
//! matcher and the old sides' hunks that carrying comments asks for.
//!
//! Must not depend on: `app`, `dispatch` or anything that runs git; this works on bytes.
//!
//! A line's bytes keep the `\n` that ends it, and a `\r` before it, so a change of line ending
//! is a change of the line. A side past [`MAX_LINES`] is not diffed.

use std::time::Instant;

use oso_core::review::{BaseHunk, Hunk};
use similar::{Algorithm, DiffOp, capture_diff_slices_deadline};

/// The most lines a side may have to be diffed.
pub(crate) const MAX_LINES: usize = 200_000;

/// How many lines `text` has, counted before any is split off.
pub(crate) fn line_count(text: &[u8]) -> usize {
    let breaks = text.iter().filter(|&&byte| byte == b'\n').count();
    breaks + usize::from(text.last().is_some_and(|&byte| byte != b'\n'))
}

/// The lines of `text`, each with its line break.
pub(crate) fn lines(text: &[u8]) -> Vec<&[u8]> {
    text.split_inclusive(|&byte| byte == b'\n').collect()
}

/// The diff's operations from `old` to `new`, or `None` once `deadline` has passed, as a diff
/// cut short reads as a replacement of nearly everything.
fn ops(old: &[&[u8]], new: &[&[u8]], deadline: Instant) -> Option<Vec<DiffOp>> {
    if Instant::now() >= deadline {
        return None;
    }
    let ops = capture_diff_slices_deadline(Algorithm::Myers, old, new, Some(deadline));
    (Instant::now() < deadline).then_some(ops)
}

/// The hunks from `old` to `new`, with no context, or `None` if either side is past
/// [`MAX_LINES`] or the diff ran past `deadline`.
pub(crate) fn hunks(old: &[&[u8]], new: &[&[u8]], deadline: Instant) -> Option<Vec<Hunk>> {
    if old.len() > MAX_LINES || new.len() > MAX_LINES {
        return None;
    }
    let ops = ops(old, new, deadline)?;
    let mut hunks = Vec::new();
    let mut open: Option<Hunk> = None;
    for op in ops {
        if let DiffOp::Equal { .. } = op {
            hunks.extend(open.take());
            continue;
        }
        let (from, to) = (op.old_range(), op.new_range());
        let hunk = open.get_or_insert_with(|| Hunk {
            old_start: number(from.start),
            new_start: number(to.start),
            removed: Vec::new(),
            added: Vec::new(),
        });
        hunk.removed
            .extend(old[from].iter().map(|line| Box::from(*line)));
        hunk.added
            .extend(new[to].iter().map(|line| Box::from(*line)));
    }
    hunks.extend(open);
    Some(hunks)
}

/// The hunks between two old sides, by line numbers, for carrying comments from a reading
/// against one to a reading against the other; `None` as [`hunks`] gives it.
pub(crate) fn base_hunks(old: &[u8], new: &[u8], deadline: Instant) -> Option<Vec<BaseHunk>> {
    if line_count(old) > MAX_LINES || line_count(new) > MAX_LINES {
        return None;
    }
    let (old, new) = (lines(old), lines(new));
    let ops = ops(&old, &new, deadline)?;
    let mut hunks: Vec<BaseHunk> = Vec::new();
    let mut open: Option<BaseHunk> = None;
    for op in ops {
        if let DiffOp::Equal { .. } = op {
            hunks.extend(open.take());
            continue;
        }
        let (from, to) = (op.old_range(), op.new_range());
        let hunk = open.get_or_insert_with(|| BaseHunk {
            old: number(from.start)..number(from.start),
            new: number(to.start)..number(to.start),
        });
        hunk.old.end = number(from.end);
        hunk.new.end = number(to.end);
    }
    hunks.extend(open);
    Some(hunks)
}

/// The pairs of equal lines a longest common subsequence of `a` and `b` gives, as carrying
/// comments matches lines put in: none once `deadline` has passed.
pub(crate) fn matches(a: &[&[u8]], b: &[&[u8]], deadline: Instant) -> Vec<(usize, usize)> {
    let Some(ops) = ops(a, b, deadline) else {
        return Vec::new();
    };
    ops.iter()
        .filter_map(|op| match *op {
            DiffOp::Equal {
                old_index,
                new_index,
                len,
            } => Some((0..len).map(move |n| (old_index + n, new_index + n))),
            _ => None,
        })
        .flatten()
        .collect()
}

/// Line number `index + 1`, held under the 32-bit limit the sides' line count keeps it below.
fn number(index: usize) -> u32 {
    u32::try_from(index + 1).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use oso_core::review::{Base, BaseHunk, Shape};
    use proptest::prelude::*;

    use super::{base_hunks, hunks, lines, matches};

    fn later() -> Instant {
        Instant::now() + Duration::from_secs(5)
    }

    #[test]
    fn hunks_have_no_context_and_keep_line_breaks() {
        let old = b"a\nb\nc\nd\n";
        let new = b"a\nB\nc\nd\ne";
        let hunks = hunks(&lines(old), &lines(new), later()).unwrap();
        assert_eq!(hunks.len(), 2);
        assert_eq!((hunks[0].old_start, hunks[0].new_start), (2, 2));
        assert_eq!(hunks[0].removed, [Box::from(&b"b\n"[..])]);
        assert_eq!(hunks[0].added, [Box::from(&b"B\n"[..])]);
        // The last line has no break, and is put in front of old line 5.
        assert_eq!((hunks[1].old_start, hunks[1].new_start), (5, 5));
        assert!(hunks[1].removed.is_empty());
        assert_eq!(hunks[1].added, [Box::from(&b"e"[..])]);
    }

    #[test]
    fn a_change_of_line_ending_is_a_change() {
        let hunks = hunks(&lines(b"a\r\nb\n"), &lines(b"a\nb\n"), later()).unwrap();
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].removed, [Box::from(&b"a\r\n"[..])]);
    }

    #[test]
    fn a_past_deadline_gives_nothing() {
        let past = Instant::now();
        assert!(hunks(&lines(b"a\n"), &lines(b"b\n"), past).is_none());
        assert!(matches(&[b"a"], &[b"a"], past).is_empty());
    }

    #[test]
    fn base_hunks_count_from_one_and_put_insertions_in_front() {
        let found = base_hunks(b"a\nb\nc\n", b"x\na\nc\n", later()).unwrap();
        assert_eq!(
            found,
            [
                BaseHunk {
                    old: 1..1,
                    new: 1..2
                },
                BaseHunk {
                    old: 2..3,
                    new: 3..3
                },
            ]
        );
    }

    #[test]
    fn matches_pair_equal_lines_in_order() {
        let a: [&[u8]; 3] = [b"x", b"y", b"z"];
        let b: [&[u8]; 3] = [b"y", b"q", b"z"];
        assert_eq!(matches(&a, &b, later()), [(1, 0), (2, 2)]);
    }

    fn side() -> impl Strategy<Value = Vec<u8>> {
        prop::collection::vec(prop::sample::select(&b"ab\n"[..]), 0..60)
    }

    proptest! {
        #[test]
        fn every_diff_makes_a_shape_that_rebuilds_the_new_side(old in side(), new in side()) {
            let (a, b) = (lines(&old), lines(&new));
            let hunks = hunks(&a, &b, later()).unwrap();
            let shape = Shape::new(Base::Unknown, hunks).unwrap();
            // Replaying the hunks over the old side gives the new one.
            let mut rebuilt = Vec::new();
            let mut next = 0;
            for hunk in shape.hunks() {
                let start = hunk.old_start as usize - 1;
                rebuilt.extend(a[next..start].iter().copied());
                rebuilt.extend(hunk.added.iter().map(|line| &**line));
                next = start + hunk.removed.len();
            }
            rebuilt.extend(a[next..].iter().copied());
            prop_assert_eq!(rebuilt.concat(), new);
        }
    }
}
