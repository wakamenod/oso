//! Tests of highlights against a model with one entry a byte.
//!
//! Must not depend on: `compose`.

use proptest::prelude::*;

use super::Highlights;
use crate::syntax::{Highlight, Splice};

type Model = Vec<Option<Highlight>>;

const KINDS: [Option<Highlight>; 4] = [
    None,
    Some(Highlight::Comment),
    Some(Highlight::String),
    Some(Highlight::Keyword),
];

/// The model of `highlights`, checking that its runs are not empty and that neighbours differ.
fn model(highlights: &Highlights) -> Model {
    let mut out = Vec::new();
    let mut last: Option<Option<Highlight>> = None;
    for (range, highlight) in highlights.runs(0..highlights.len()) {
        assert!(!range.is_empty(), "an empty run");
        assert_ne!(last, Some(highlight), "neighbours with one highlight");
        last = Some(highlight);
        out.extend(std::iter::repeat_n(highlight, range.len()));
    }
    assert_eq!(out.len(), highlights.len());
    out
}

#[test]
fn a_range_reads_its_runs_cut_to_it() {
    let mut highlights = Highlights::new(10);
    highlights.replace(
        2..8,
        [(3, Some(Highlight::String)), (3, Some(Highlight::Comment))],
    );
    let runs: Vec<_> = highlights.runs(3..7).collect();
    assert_eq!(
        runs,
        [
            (3..5, Some(Highlight::String)),
            (5..7, Some(Highlight::Comment)),
        ]
    );
    assert_eq!(highlights.runs(10..20).count(), 0);
    assert_eq!(highlights.at(9), None);
    assert_eq!(highlights.at(2), Some(Highlight::String));
}

#[test]
fn typed_text_takes_the_highlight_before_it() {
    let mut highlights = Highlights::new(10);
    highlights.replace(0..4, [(4, Some(Highlight::String))]);
    // Inside the string, at its end, and at the start of the buffer.
    let splices = [
        Splice {
            start: 0,
            old_len: 0,
            new_len: 1,
        },
        Splice {
            start: 2,
            old_len: 0,
            new_len: 1,
        },
        Splice {
            start: 4,
            old_len: 1,
            new_len: 2,
        },
    ];
    highlights.splice(&splices);
    let s = Some(Highlight::String);
    assert_eq!(
        model(&highlights),
        [None, s, s, s, s, s, s, s, None, None, None, None, None]
    );
}

#[derive(Clone, Debug)]
enum Op {
    Replace(usize, usize, Vec<(usize, usize)>),
    Splice(Vec<(usize, usize, usize)>),
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (
            0usize..40,
            0usize..20,
            prop::collection::vec((0usize..8, 0..KINDS.len()), 0..5)
        )
            .prop_map(|(start, len, runs)| Op::Replace(start, len, runs)),
        prop::collection::vec((0usize..10, 0usize..6, 0usize..6), 0..4).prop_map(Op::Splice),
    ]
}

proptest! {
    #[test]
    fn highlights_do_what_a_byte_by_byte_model_does(
        len in 0usize..40,
        ops in prop::collection::vec(op(), 0..12),
    ) {
        let mut highlights = Highlights::new(len);
        let mut expected: Model = vec![None; len];
        for op in ops {
            match op {
                Op::Replace(start, run_len, runs) => {
                    let start = start.min(expected.len());
                    let end = (start + run_len).min(expected.len());
                    let runs: Vec<_> = runs.into_iter().map(|(n, k)| (n, KINDS[k])).collect();
                    highlights.replace(start..end, runs.clone());
                    let mut middle = Vec::new();
                    for (n, kind) in runs {
                        let n = n.min(end - start - middle.len());
                        middle.extend(std::iter::repeat_n(kind, n));
                    }
                    middle.resize(end - start, None);
                    expected.splice(start..end, middle);
                }
                Op::Splice(drawn) => {
                    let mut splices = Vec::new();
                    let mut at = 0;
                    for (gap, old_len, new_len) in drawn {
                        let start = at + gap;
                        if start > expected.len() {
                            break;
                        }
                        let old_len = old_len.min(expected.len() - start);
                        splices.push(Splice { start, old_len, new_len });
                        at = start + old_len;
                    }
                    highlights.splice(&splices);
                    for splice in splices.iter().rev() {
                        let guess = splice.start.checked_sub(1).and_then(|b| expected[b]);
                        expected.splice(
                            splice.start..splice.start + splice.old_len,
                            std::iter::repeat_n(guess, splice.new_len),
                        );
                    }
                }
            }
            prop_assert_eq!(model(&highlights), expected.clone());
            prop_assert!(whole_runs_hold_each_byte(&highlights, &expected));
        }
    }
}

/// Whether `run_at` gives, for each byte, all the bytes around it that `expected` says are
/// the same, and nothing past the end.
fn whole_runs_hold_each_byte(highlights: &Highlights, expected: &Model) -> bool {
    let each = (0..expected.len()).all(|at| {
        let same = |b: &usize| expected[*b] == expected[at];
        let start = (0..at).rev().take_while(same).last().unwrap_or(at);
        let end = (at..expected.len())
            .take_while(same)
            .last()
            .map_or(at, |b| b + 1);
        highlights.run_at(at) == Some((start..end, expected[at]))
    });
    each && highlights.run_at(expected.len()).is_none()
}
