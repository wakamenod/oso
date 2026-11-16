//! Tests of placing a diagnostics report against the text it names and mapping it to the latest
//! text given: versions, a report that names none, ones that are past, the bound on the texts
//! kept, and random batches against a scan of their own.
//!
//! Must not depend on: the session's loop or a process.

#![expect(
    clippy::single_range_in_vec_init,
    reason = "a set of one range is what most of these check"
)]

use oso_core::diagnostic::{Severity, Tags};
use oso_core::rope::Rope;
use oso_core::syntax::Splice;
use proptest::prelude::*;
use std::path::{Path, PathBuf};

use super::*;
use crate::position::Position;
use crate::protocol::Reported;
use crate::server::Document;

fn open(documents: &mut Documents, text: &str) {
    documents.open(
        Document {
            id: 1,
            uri: "file:///w/a.rs".into(),
            language_id: "rust".into(),
            text: Rope::from(text),
            revision: 10,
        },
        true,
    );
    while documents.next_whole().is_some() {}
}

/// Changes document 1 from `before` to `after` by `splices`, as revision `revision`.
fn change(
    documents: &mut Documents,
    revision: u64,
    before: &str,
    after: &str,
    splices: Option<&[Splice]>,
) {
    documents.change(
        1,
        revision,
        &Rope::from(before),
        Rope::from(after),
        splices,
        Encoding::Utf8,
        true,
    );
    while documents.next_whole().is_some() {}
}

fn report(version: Option<i32>, ranges: &[(Position, Position)]) -> Published {
    Published {
        uri: "file:///w/a.rs".into(),
        version,
        diagnostics: ranges
            .iter()
            .map(|&(start, end)| Reported {
                start,
                end,
                severity: Severity::Error,
                tags: Tags::default(),
                source: String::new(),
                code: String::new(),
                message: "m".into(),
            })
            .collect(),
    }
}

fn placed(diagnose: Diagnose) -> (u64, Vec<std::ops::Range<usize>>) {
    match diagnose {
        Diagnose::For {
            id,
            revision,
            diagnostics,
        } => {
            assert_eq!(id, 1);
            (
                revision,
                diagnostics
                    .iter()
                    .map(|diagnostic| diagnostic.range())
                    .collect(),
            )
        }
        other => panic!("not placed: {other:?}"),
    }
}

const X: (Position, Position) = (Position::new(0, 4), Position::new(0, 5));

fn insert(start: usize, new_len: usize) -> Splice {
    Splice {
        start,
        old_len: 0,
        new_len,
    }
}

#[test]
fn a_report_for_an_older_version_is_mapped_to_the_latest_text() {
    let mut documents = Documents::default();
    open(&mut documents, "let x = 1;\n");
    change(
        &mut documents,
        11,
        "let x = 1;\n",
        "// a\nlet x = 1;\n",
        Some(&[insert(0, 5)]),
    );
    let (revision, ranges) = placed(documents.diagnose(
        Path::new("/w/a.rs"),
        &report(Some(1), &[X]),
        "fake",
        Encoding::Utf8,
    ));
    assert_eq!((revision, ranges), (11, vec![9..10]));
    assert_eq!(
        documents.given(1),
        2,
        "the text of version 1 stays until one past it comes"
    );
    // Version 2 names the new text, and lets version 1's go.
    let line_1 = (Position::new(1, 4), Position::new(1, 5));
    let (_, ranges) = placed(documents.diagnose(
        Path::new("/w/a.rs"),
        &report(Some(2), &[line_1]),
        "fake",
        Encoding::Utf8,
    ));
    assert_eq!(ranges, vec![9..10]);
    assert_eq!(documents.given(1), 1);
    assert!(matches!(
        documents.diagnose(
            Path::new("/w/a.rs"),
            &report(Some(1), &[X]),
            "fake",
            Encoding::Utf8
        ),
        Diagnose::Stale
    ));
}

#[test]
fn a_report_for_no_version_is_placed_against_the_latest_sent() {
    let mut documents = Documents::default();
    open(&mut documents, "let x = 1;\n");
    change(
        &mut documents,
        11,
        "let x = 1;\n",
        "let xy = 1;\n",
        Some(&[insert(5, 1)]),
    );
    let (revision, ranges) = placed(documents.diagnose(
        Path::new("/w/a.rs"),
        &report(None, &[X]),
        "fake",
        Encoding::Utf8,
    ));
    assert_eq!((revision, ranges), (11, vec![4..5]));
    // Past a whole text, a versioned one is past, and one for no version takes the latest.
    change(&mut documents, 12, "let xy = 1;\n", "fn x() {}\n", None);
    assert!(matches!(
        documents.diagnose(
            Path::new("/w/a.rs"),
            &report(Some(2), &[X]),
            "fake",
            Encoding::Utf8
        ),
        Diagnose::Stale
    ));
    let (revision, ranges) = placed(documents.diagnose(
        Path::new("/w/a.rs"),
        &report(None, &[X]),
        "fake",
        Encoding::Utf8,
    ));
    assert_eq!((revision, ranges), (12, vec![4..5]));
}

#[test]
fn a_report_for_a_document_not_open_or_a_version_never_sent_is_not_placed() {
    let mut documents = Documents::default();
    assert!(matches!(
        documents.diagnose(
            Path::new("/w/a.rs"),
            &report(None, &[X]),
            "fake",
            Encoding::Utf8
        ),
        Diagnose::NotOpen
    ));
    documents.open(
        Document {
            id: 1,
            uri: "file:///w/a.rs".into(),
            language_id: "rust".into(),
            text: Rope::from("a"),
            revision: 0,
        },
        false,
    );
    assert!(matches!(
        documents.diagnose(
            Path::new("/w/a.rs"),
            &report(None, &[X]),
            "fake",
            Encoding::Utf8
        ),
        Diagnose::NotOpen
    ));
    assert!(documents.has_path(Path::new("/w/a.rs")));
    documents.open_all();
    while documents.next_whole().is_some() {}
    assert_eq!(documents.take_opened(), [PathBuf::from("/w/a.rs")]);
    assert!(matches!(
        documents.diagnose(
            Path::new("/w/a.rs"),
            &report(Some(9), &[X]),
            "fake",
            Encoding::Utf8
        ),
        Diagnose::Stale
    ));
}

#[test]
fn at_most_sixteen_texts_are_kept() {
    let mut documents = Documents::default();
    open(&mut documents, "");
    let mut text = String::new();
    for n in 0..20 {
        let before = text.clone();
        text.push('a');
        change(
            &mut documents,
            11 + n,
            &before,
            &text,
            Some(&[insert(n as usize, 1)]),
        );
    }
    assert_eq!(documents.given(1), KEPT_VERSIONS_MAX);
    assert!(matches!(
        documents.diagnose(
            Path::new("/w/a.rs"),
            &report(Some(1), &[]),
            "fake",
            Encoding::Utf8
        ),
        Diagnose::Stale
    ));
    let (revision, _) = placed(documents.diagnose(
        Path::new("/w/a.rs"),
        &report(Some(10), &[]),
        "fake",
        Encoding::Utf8,
    ));
    assert_eq!(revision, 30);
}

fn text_strategy() -> impl Strategy<Value = String> {
    prop::collection::vec(
        prop::sample::select(vec!["a", "é", "日", "\u{1F600}", "\n", " "]),
        0..40,
    )
    .prop_map(|parts| parts.concat())
}

/// Edits of `text` that are ascending and apart, from random picks, as `(splices, after)`.
fn edit(text: &str, picks: &[(usize, usize, String)]) -> (Vec<Splice>, String) {
    let mut splices = Vec::new();
    let mut after = String::new();
    let mut at = 0;
    for (gap, len, insert) in picks {
        let start = text.floor_char_boundary((at + gap).min(text.len()));
        if start < at || (start == at && !splices.is_empty()) {
            continue;
        }
        let end = text.floor_char_boundary((start + len).min(text.len()));
        after.push_str(&text[at..start]);
        after.push_str(insert);
        splices.push(Splice {
            start,
            old_len: end - start,
            new_len: insert.len(),
        });
        at = end;
    }
    after.push_str(&text[at..]);
    (splices, after)
}

proptest! {
    /// A range that no batch since its version touched covers the same text in the latest.
    #[test]
    fn an_untouched_range_covers_the_same_text_after_random_batches(
        first in text_strategy(),
        ranges in prop::collection::vec((0usize..80, 0usize..8), 1..6),
        batches in prop::collection::vec(
            prop::collection::vec((1usize..10, 0usize..4, text_strategy()), 0..4),
            1..5,
        ),
        utf16 in any::<bool>(),
    ) {
        let encoding = if utf16 { Encoding::Utf16 } else { Encoding::Utf8 };
        let mut documents = Documents::default();
        open(&mut documents, &first);
        let rope = Rope::from(first.as_str());
        let ranges: Vec<std::ops::Range<usize>> = ranges
            .iter()
            .map(|&(start, width)| {
                let start = first.floor_char_boundary(start.min(first.len()));
                start..first.floor_char_boundary((start + width).min(first.len()))
            })
            .collect();
        let positions: Vec<(Position, Position)> = ranges
            .iter()
            .map(|range| {
                (
                    position::from_offset(&rope, range.start, encoding),
                    position::from_offset(&rope, range.end, encoding),
                )
            })
            .collect();
        // Each range followed by a scan of its own: `None` once a splice meets it.
        let mut followed: Vec<Option<std::ops::Range<usize>>> =
            ranges.iter().cloned().map(Some).collect();
        let mut text = first.clone();
        for (n, picks) in batches.iter().enumerate() {
            let (splices, after) = edit(&text, picks);
            for range in &mut followed {
                *range = range.take().and_then(|range| {
                    let mut shift = 0isize;
                    for splice in &splices {
                        if splice.start <= range.end && splice.start + splice.old_len >= range.start {
                            return None;
                        }
                        if splice.start < range.start {
                            shift += splice.new_len as isize - splice.old_len as isize;
                        }
                    }
                    Some(range.start.saturating_add_signed(shift)..range.end.saturating_add_signed(shift))
                });
            }
            change(&mut documents, 11 + n as u64, &text, &after, Some(&splices));
            text = after;
        }
        let Diagnose::For { revision, diagnostics, .. } =
            documents.diagnose(Path::new("/w/a.rs"), &report(Some(1), &positions), "fake", encoding)
        else {
            return Err(TestCaseError::fail("not placed"));
        };
        prop_assert_eq!(revision, 10 + batches.len() as u64);
        prop_assert!(diagnostics.fits(text.len()));
        let mut expected: Vec<&str> = ranges
            .iter()
            .zip(&followed)
            .filter_map(|(original, now)| now.as_ref().map(|_| &first[original.clone()]))
            .collect();
        let mut got: Vec<&str> = diagnostics
            .iter()
            .map(|diagnostic| &text[diagnostic.range()])
            .collect();
        // Every untouched range's text is among those placed.
        for wanted in expected.drain(..) {
            let at = got.iter().position(|placed| *placed == wanted);
            prop_assert!(at.is_some(), "{:?} not in {:?}", wanted, got);
            got.remove(at.unwrap_or_default());
        }
    }
}
