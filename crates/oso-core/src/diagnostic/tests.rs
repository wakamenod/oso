//! Tests of a buffer's diagnostics as a set: sorting, strings, the ranges found by binary
//! search, the moves, and following batches of edits, against a scan of their own.
//!
//! Must not depend on: `buffer`, `editor` or `window`.

#![expect(
    clippy::single_range_in_vec_init,
    reason = "a set of one range is what many of these check"
)]

use proptest::prelude::*;

use super::*;

fn new(range: Range<usize>, severity: Severity, message: &str) -> New<'_> {
    New {
        range,
        severity,
        tags: Tags::default(),
        source: "rustc",
        code: "E0308",
        message,
    }
}

fn set(entries: &[(Range<usize>, Severity)], len: usize) -> Diagnostics {
    Diagnostics::new(
        "rust-analyzer",
        entries
            .iter()
            .map(|(range, severity)| new(range.clone(), *severity, "m")),
        len,
    )
}

fn ranges(set: &Diagnostics) -> Vec<Range<usize>> {
    set.iter().map(|diagnostic| diagnostic.range()).collect()
}

fn splice(start: usize, old_len: usize, new_len: usize) -> Splice {
    Splice {
        start,
        old_len,
        new_len,
    }
}

#[test]
fn a_set_is_sorted_by_start_then_severity_and_keeps_its_strings() {
    let set = Diagnostics::new(
        "rust-analyzer",
        [
            new(5..9, Severity::Warning, "unused\nsecond line"),
            new(1..3, Severity::Hint, "hint"),
            new(5..6, Severity::Error, "mismatched types"),
        ],
        20,
    );
    assert_eq!(ranges(&set), [1..3, 5..6, 5..9]);
    assert_eq!(set.server(), "rust-analyzer");
    let error = set.get(1).unwrap();
    assert_eq!(error.severity(), Severity::Error);
    assert_eq!(error.message(), "mismatched types");
    assert_eq!((error.source(), error.code()), ("rustc", "E0308"));
    assert_eq!(set.get(2).unwrap().first_line(), "unused");
    assert_eq!(
        set.counts(),
        Counts {
            errors: 1,
            warnings: 1,
            information: 0
        }
    );
}

#[test]
fn ranges_are_cut_to_the_text_and_debug_shows_no_message() {
    let set = Diagnostics::new(
        "s",
        [
            new(30..40, Severity::Error, "secret"),
            new(Range { start: 8, end: 2 }, Severity::Error, "x"),
        ],
        10,
    );
    assert_eq!(ranges(&set), [8..8, 10..10]);
    assert!(set.fits(10));
    assert!(!format!("{set:?}").contains("secret"));
    assert!(!format!("{:?}", set.get(0).unwrap()).contains('x'));
}

#[test]
fn in_range_finds_overlapping_and_empty_entries_past_a_long_one() {
    // A long range first, then short ones, so the starts alone would not find it.
    let set = set(
        &[
            (0..100, Severity::Warning),
            (10..12, Severity::Error),
            (50..50, Severity::Error),
            (60..70, Severity::Error),
        ],
        200,
    );
    let meet = |range: Range<usize>| -> Vec<Range<usize>> {
        set.in_range(range)
            .map(|diagnostic| diagnostic.range())
            .collect()
    };
    assert_eq!(meet(40..60), [0..100, 50..50]);
    assert_eq!(meet(12..20), [0..100]);
    assert_eq!(meet(100..150), Vec::<Range<usize>>::new());
    assert_eq!(meet(50..50), [0..100, 50..50]);
    let at = |offset| -> Vec<Range<usize>> {
        set.at(offset)
            .map(|diagnostic| diagnostic.range())
            .collect()
    };
    assert_eq!(at(11), [0..100, 10..12]);
    assert_eq!(at(12), [0..100]);
    assert_eq!(at(70), [0..100]);
    assert_eq!(at(100), Vec::<Range<usize>>::new());
}

#[test]
fn a_step_skips_hints_and_wraps_past_either_end() {
    let all = set(
        &[
            (2..3, Severity::Hint),
            (5..6, Severity::Error),
            (9..10, Severity::Information),
            (12..13, Severity::Hint),
        ],
        20,
    );
    let step = |offset, forward| {
        all.step(offset, forward)
            .map(|(diagnostic, wrapped)| (diagnostic.range().start, wrapped))
    };
    assert_eq!(step(0, true), Some((5, false)));
    assert_eq!(step(5, true), Some((9, false)));
    assert_eq!(step(9, true), Some((5, true)), "past the last, the first");
    assert_eq!(step(9, false), Some((5, false)));
    assert_eq!(
        step(5, false),
        Some((9, true)),
        "before the first, the last"
    );
    assert_eq!(step(20, false), Some((9, false)));
    let hints = set(&[(1..2, Severity::Hint)], 5);
    assert!(hints.step(0, true).is_none());
    assert!(Diagnostics::new("s", [], 0).step(0, false).is_none());
}

#[test]
fn a_first_line_ends_at_any_line_break() {
    for (message, line) in [
        ("a\nb", "a"),
        ("a\rb", "a"),
        ("a\u{85}b", "a"),
        ("a\u{2028}b", "a"),
        ("a\u{2029}b", "a"),
        ("", ""),
    ] {
        let set = Diagnostics::new("s", [new(0..1, Severity::Error, message)], 1);
        assert_eq!(set.get(0).unwrap().first_line(), line, "{message:?}");
    }
}

#[test]
fn text_inserted_at_an_edge_stays_outside_and_inside_grows_it() {
    let mut at_start = set(&[(5..10, Severity::Error)], 20);
    at_start.splice(&[splice(5, 0, 3)]);
    assert_eq!(ranges(&at_start), [8..13]);
    let mut at_end = set(&[(5..10, Severity::Error)], 20);
    at_end.splice(&[splice(10, 0, 3)]);
    assert_eq!(ranges(&at_end), [5..10]);
    let mut inside = set(&[(5..10, Severity::Error)], 20);
    inside.splice(&[splice(7, 0, 3)]);
    assert_eq!(ranges(&inside), [5..13]);
}

#[test]
fn a_replacement_of_its_text_stays_in_and_one_around_it_empties_it() {
    let mut whole = set(&[(5..10, Severity::Error)], 20);
    whole.splice(&[splice(5, 5, 2)]);
    assert_eq!(ranges(&whole), [5..7]);
    let mut around = set(&[(5..10, Severity::Error)], 20);
    around.splice(&[splice(3, 9, 0)]);
    assert_eq!(ranges(&around), [3..3]);
    let mut around_typed = set(&[(5..10, Severity::Error)], 20);
    around_typed.splice(&[splice(3, 9, 4)]);
    assert_eq!(ranges(&around_typed), [7..7]);
    let mut tail = set(&[(5..10, Severity::Error)], 20);
    tail.splice(&[splice(8, 4, 1)]);
    assert_eq!(ranges(&tail), [5..8]);
}

#[test]
fn many_cursors_shift_each_entry_by_the_splices_before_it() {
    let mut set = set(
        &[
            (0..1, Severity::Error),
            (10..12, Severity::Error),
            (20..21, Severity::Error),
        ],
        30,
    );
    set.splice(&[splice(2, 0, 1), splice(5, 1, 0), splice(15, 0, 4)]);
    assert_eq!(ranges(&set), [0..1, 10..12, 24..25]);
    assert!(set.fits(34));
}

/// Where an offset goes by itself through the batch, splice by splice from the highest, with
/// the same rules, as a check of the binary search and the shifts.
fn naive(offset: usize, splices: &[Splice], start: bool) -> usize {
    let mut offset = offset;
    for splice in splices.iter().rev() {
        let old_end = splice.start + splice.old_len;
        offset = if offset < splice.start || (!start && offset == splice.start) {
            offset
        } else if start && offset == splice.start && splice.old_len > 0 {
            splice.start
        } else if offset > old_end {
            offset + splice.new_len - splice.old_len
        } else if offset == old_end {
            if start || splice.old_len > 0 {
                splice.start + splice.new_len
            } else {
                offset
            }
        } else if start {
            splice.start + splice.new_len
        } else {
            splice.start
        };
    }
    offset
}

fn batch() -> impl Strategy<Value = (usize, Vec<Splice>)> {
    prop::collection::vec((1usize..8, 0usize..5, 0usize..5), 0..12).prop_map(|steps| {
        let (mut at, mut splices) = (0, Vec::new());
        for (gap, old_len, new_len) in steps {
            let start = if splices.is_empty() {
                gap - 1
            } else {
                at + gap
            };
            splices.push(splice(start, old_len, new_len));
            at = start + old_len;
        }
        (at + 4, splices)
    })
}

proptest! {
    #[test]
    fn ranges_follow_a_batch_as_each_offset_would(
        (len, splices) in batch(),
        entries in prop::collection::vec((0usize..60, 0usize..20, 0u8..4), 0..20),
    ) {
        let severities = [Severity::Error, Severity::Warning, Severity::Information, Severity::Hint];
        let entries: Vec<(Range<usize>, Severity)> = entries
            .into_iter()
            .map(|(start, width, severity)| {
                let start = start.min(len);
                (start..(start + width).min(len), severities[usize::from(severity)])
            })
            .collect();
        let mut followed = set(&entries, len);
        followed.splice(&splices);
        let mut expected: Vec<Range<usize>> = entries
            .iter()
            .map(|(range, _)| {
                let start = naive(range.start, &splices, true);
                start..naive(range.end, &splices, false).max(start)
            })
            .collect();
        expected.sort_by_key(|range| range.start);
        let mut got = ranges(&followed);
        got.sort_by_key(|range| (range.start, range.end));
        expected.sort_by_key(|range| (range.start, range.end));
        prop_assert_eq!(got, expected);
        let shift: isize = splices.iter().map(|s| s.new_len as isize - s.old_len as isize).sum();
        prop_assert!(followed.fits(len.saturating_add_signed(shift)));
        // Still sorted, and found by in_range as a scan finds them.
        let starts: Vec<usize> = followed.iter().map(|d| d.range().start).collect();
        prop_assert!(starts.windows(2).all(|pair| pair[0] <= pair[1]));
        for probe in 0..len + 4 {
            let found = followed.at(probe).count();
            let scanned = followed.iter().filter(|d| {
                let r = d.range();
                r.start <= probe && (probe < r.end || r.is_empty() && r.start == probe)
            }).count();
            prop_assert_eq!(found, scanned);
        }
    }
}
