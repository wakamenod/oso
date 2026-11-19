//! The unchanged lines around a file's hunks: which show, how hunks whose context meets join,
//! what the person asked to see and how long it is kept, both panes, and the files that show
//! none.
//!
//! Must not depend on: `app`, `dispatch` or winit; these lay out readings made here.

use std::sync::Arc;
use std::time::{Duration, Instant};

use oso_core::review::{Base, Rows, Shape, Side};
use proptest::prelude::*;

use super::super::cards::Cards;
use super::super::file::File;
use super::super::section::Rendered;
use super::super::section::tests::{entry, lines_of};
use super::super::{Carrying, Tell};
use super::*;
use crate::review::list::Status;
use crate::review::refine::Refined;
use crate::review::{Content, Reading, diff};

fn later() -> Instant {
    Instant::now() + Duration::from_secs(5)
}

/// `count` lines, `l1` to `l<count>`, with line `n` of `changed` changed to `L<n>`.
fn numbered(count: u32, changed: &[u32]) -> String {
    (1..=count)
        .map(|n| {
            if changed.contains(&n) {
                format!("L{n}\n")
            } else {
                format!("l{n}\n")
            }
        })
        .collect()
}

/// A text reading of `a.txt` from `old` to `new`, diffed as a review diffs it, against the old
/// side `base`.
fn reading_of(old: impl AsRef<[u8]>, new: impl AsRef<[u8]>, base: &[u8]) -> Reading {
    let (o, n) = (old.as_ref(), new.as_ref());
    let hunks = diff::hunks(&diff::lines(o), &diff::lines(n), later()).unwrap();
    Reading {
        entry: entry(b"a.txt", Status::Modified),
        content: Content::Text {
            refined: Refined::of(&hunks, later()),
            shape: Arc::new(Shape::new(Base::Blob(base.into()), hunks).unwrap()),
            old: Arc::from(o),
            new: Arc::from(n),
        },
    }
}

/// A view's file showing `reading`.
fn file_of(reading: Reading) -> File {
    let mut file = File::new(reading.entry.clone(), Cards::default());
    file.show(Arc::new(reading), None, false);
    file
}

/// Each line's text, with the old and new numbers of a context row or the gap of a gap's row.
fn texts(rendered: &Rendered) -> Vec<String> {
    lines_of(rendered)
        .into_iter()
        .map(|(text, kind)| match kind {
            RowKind::Context { .. } => format!("{text}|ctx"),
            RowKind::Gap { old, new, lines } => format!("{text}|gap {old} {new} {lines}"),
            _ => text,
        })
        .collect()
}

#[test]
fn three_lines_show_around_a_hunk_and_a_gap_row_stands_for_the_rest() {
    let file = file_of(reading_of(numbered(20, &[]), numbered(20, &[10]), b"a"));
    assert_eq!(
        texts(&file.render(usize::MAX, false)),
        [
            "modified  a.txt  +1 \u{2212}1",
            "\u{22ef} 6 unchanged lines|gap 1 1 6",
            " l7|ctx",
            " l8|ctx",
            " l9|ctx",
            "@@ -10 +10 @@",
            "-l10",
            "+L10",
            " l11|ctx",
            " l12|ctx",
            " l13|ctx",
            "\u{22ef} 7 unchanged lines|gap 14 14 7",
        ]
    );
}

#[test]
fn hunks_whose_context_meets_show_the_lines_between_once_and_keep_their_headers() {
    // Six lines apart: each hunk's three meet.
    let file = file_of(reading_of(numbered(12, &[]), numbered(12, &[2, 9]), b"a"));
    assert_eq!(
        texts(&file.render(usize::MAX, false)),
        [
            "modified  a.txt  +2 \u{2212}2",
            " l1|ctx",
            "@@ -2 +2 @@",
            "-l2",
            "+L2",
            " l3|ctx",
            " l4|ctx",
            " l5|ctx",
            " l6|ctx",
            " l7|ctx",
            " l8|ctx",
            "@@ -9 +9 @@",
            "-l9",
            "+L9",
            " l10|ctx",
            " l11|ctx",
            " l12|ctx",
        ]
    );
    // Seven apart, the one line between their context shows too, as its row would take its
    // room; eight apart, two hide under a gap's row.
    let file = file_of(reading_of(numbered(14, &[]), numbered(14, &[2, 10]), b"a"));
    let shown = texts(&file.render(usize::MAX, false));
    assert!(shown.contains(&" l6|ctx".to_owned()));
    assert!(!shown.iter().any(|text| text.contains("|gap")));
    let file = file_of(reading_of(numbered(14, &[]), numbered(14, &[2, 11]), b"a"));
    let shown = texts(&file.render(usize::MAX, false));
    assert!(shown.contains(&"\u{22ef} 2 unchanged lines|gap 6 6 2".to_owned()));
}

#[test]
fn context_lines_are_numbered_on_both_sides_and_visit_the_new_one() {
    // A line put in before them moves the new side's numbers by one.
    let old = numbered(10, &[]);
    let new = format!("l0\n{old}");
    let file = file_of(reading_of(&old, &new, b"a"));
    let rendered = file.render(usize::MAX, false);
    let rows = Rows::new(vec![Arc::clone(&rendered.section)]);
    // The header, the hunk's header and its line, then the context.
    let row = rows.at(3).unwrap();
    assert_eq!(row.context(), Some((1, 2)));
    assert_eq!(row.line(), Some((Side::New, 2)));
    let slot = rendered.slots.iter().find(|slot| slot.number == 2).unwrap();
    assert_eq!(slot.side, Side::New);
}

#[test]
fn context_lines_lose_their_line_endings_and_keep_invalid_bytes() {
    let old = b"a\r\nb\r\nc\r\n\xff\r\n".to_vec();
    let mut new = old.clone();
    new[0] = b'A';
    let file = file_of(reading_of(&old, &new, b"a"));
    let rendered = file.render(usize::MAX, false);
    assert!(!rendered.valid);
    let shown = texts(&rendered);
    assert_eq!(shown[4], " b|ctx");
    assert_eq!(shown[5], " c|ctx");
    // The byte goes in as it is, for the loader to show as `\xNN`.
    assert_eq!(shown[6], " \u{fffd}|ctx");
}

#[test]
fn lines_asked_for_show_and_stay_across_a_reading_against_the_same_old_side() {
    let old = numbered(40, &[]);
    let mut file = file_of(reading_of(&old, numbered(40, &[20]), b"a"));
    file.revealed.add(5..8, &Base::Blob(b"a".to_vec().into()));
    let shown = texts(&file.render(usize::MAX, false));
    assert_eq!(
        &shown[1..6],
        [
            "\u{22ef} 4 unchanged lines|gap 1 1 4",
            " l5|ctx",
            " l6|ctx",
            " l7|ctx",
            "\u{22ef} 9 unchanged lines|gap 8 8 9",
        ]
    );
    // The agent changed another line: the same old lines still show.
    file.show(
        Arc::new(reading_of(&old, numbered(40, &[20, 30]), b"a")),
        None,
        false,
    );
    assert!(texts_contain(&file, " l5|ctx"));
    // A reading against another old side drops them.
    file.show(
        Arc::new(reading_of(&old, numbered(40, &[20]), b"b")),
        None,
        false,
    );
    assert!(!texts_contain(&file, " l5|ctx"));
}

fn texts_contain(file: &File, line: &str) -> bool {
    texts(&file.render(usize::MAX, false))
        .iter()
        .any(|text| text == line)
}

#[test]
fn what_is_asked_joins_what_it_meets_and_is_bounded() {
    let base = Base::Blob(b"a".to_vec().into());
    let mut revealed = Revealed::default();
    revealed.add(10..20, &base);
    revealed.add(30..40, &base);
    revealed.add(20..30, &base);
    assert_eq!(revealed.against(&base), std::slice::from_ref(&(10..40)));
    revealed.add(1..2, &base);
    assert_eq!(revealed.against(&base), [1..2, 10..40]);
    assert!(revealed.against(&Base::Unknown).is_empty());
    for n in 0..1000 {
        revealed.add(100 + n * 2..101 + n * 2, &base);
    }
    assert!(revealed.against(&base).len() <= MAX_REVEALED);
    revealed.clear();
    assert!(revealed.against(&base).is_empty());
}

#[test]
fn files_not_read_as_text_and_files_with_no_hunk_show_no_context() {
    for content in [
        Content::Withheld,
        Content::Binary { old: 1, new: 2 },
        Content::TooLarge { old: 1, new: 2 },
        Content::Conflicted,
    ] {
        let file = file_of(Reading {
            entry: entry(b"a.txt", Status::Modified),
            content,
        });
        let kinds = lines_of(&file.render(usize::MAX, false));
        assert!(
            kinds
                .iter()
                .all(|(_, kind)| !matches!(kind, RowKind::Context { .. } | RowKind::Gap { .. })),
            "{kinds:?}"
        );
    }
    let same = numbered(10, &[]);
    let file = file_of(reading_of(&same, &same, b"a"));
    assert_eq!(texts(&file.render(usize::MAX, false)).len(), 1);
}

#[test]
fn both_panes_show_context_row_for_row_by_their_own_side() {
    let old = numbered(20, &[]);
    let new = format!("l0\n{}", numbered(20, &[10]));
    let file = file_of(reading_of(&old, &new, b"a"));
    let sides = file.render_panes(usize::MAX, false);
    let (old_lines, new_lines) = (lines_of(&sides.old), lines_of(&sides.new));
    assert_eq!(old_lines.len(), new_lines.len());
    for ((old_text, old_kind), (new_text, new_kind)) in old_lines.iter().zip(&new_lines) {
        if let RowKind::Context { .. } | RowKind::Gap { .. } = old_kind {
            assert_eq!(old_kind, new_kind);
            assert_eq!(old_text, new_text);
        }
    }
    let old_slot = sides
        .old
        .slots
        .iter()
        .find(|slot| slot.number == 9)
        .unwrap();
    assert_eq!(old_slot.side, Side::Old);
    let new_slot = sides
        .new
        .slots
        .iter()
        .find(|slot| slot.number == 10)
        .unwrap();
    assert_eq!(new_slot.side, Side::New);
}

fn side() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(prop::sample::select(&b"ab\n"[..]), 0..80)
}

proptest! {
    /// Every unchanged line of the new side shows once, as context or under a gap's row, in
    /// order, whatever was asked to show.
    #[test]
    fn every_unchanged_line_is_shown_or_hidden_once(
        old in side(),
        new in side(),
        asked in prop::collection::vec((1u32..60, 0u32..30), 0..4),
    ) {
        let (a, b) = (diff::lines(&old), diff::lines(&new));
        let hunks = diff::hunks(&a, &b, later()).unwrap();
        prop_assume!(!hunks.is_empty());
        let lines = Lines::of(&new);
        let base = Base::Blob(b"a".to_vec().into());
        let mut revealed = Revealed::default();
        for (start, len) in asked {
            revealed.add(start..start + len, &base);
        }
        let around = Around { new: &new, lines: &lines, revealed: revealed.against(&base) };
        let mut covered = Vec::new();
        for index in 0..=hunks.len() {
            for piece in pieces(&hunks, index, &around) {
                let (Piece::Shown { new, lines, .. } | Piece::Hidden { new, lines, .. }) = piece;
                covered.extend(new..new + lines);
            }
        }
        let changed: Vec<u32> = hunks
            .iter()
            .flat_map(|hunk| hunk.new_start..hunk.new_start + hunk.added.len() as u32)
            .collect();
        let mut all: Vec<u32> = covered.iter().copied().chain(changed).collect();
        all.sort_unstable();
        prop_assert_eq!(all, (1..=b.len() as u32).collect::<Vec<_>>());
        // And in order.
        prop_assert!(covered.windows(2).all(|pair| pair[0] < pair[1]));
    }
}

/// The review text's lines once the view took `told`.
fn built_lines(told: Vec<Tell>) -> Vec<String> {
    let mut state = State::new("t".into());
    for tell in told {
        state.take(tell);
    }
    state.build();
    state.text.to_string().lines().map(str::to_owned).collect()
}

#[test]
fn the_view_shows_what_it_is_told_to_and_forgets_it_when_told_so() {
    let reading = Arc::new(reading_of(numbered(60, &[]), numbered(60, &[30]), b"a"));
    let listed = || Tell::Listed {
        entries: vec![reading.entry.clone()].into(),
        more: 0,
        cut: false,
    };
    let file = || Tell::File {
        reading: Arc::clone(&reading),
        carry: Carrying::Nothing,
    };
    let context = |show| Tell::Context {
        path: b"a.txt"[..].into(),
        show,
    };
    assert!(!built_lines(vec![listed(), file()]).contains(&" l10".to_owned()));
    let shown = built_lines(vec![listed(), file(), context(Some(10..12))]);
    assert!(shown.contains(&" l10".to_owned()) && shown.contains(&" l11".to_owned()));
    let reset = built_lines(vec![listed(), file(), context(Some(10..12)), context(None)]);
    assert!(!reset.contains(&" l10".to_owned()));
    // A file the view does not show is let be.
    let other = Tell::Context {
        path: b"b.txt"[..].into(),
        show: Some(1..5),
    };
    assert_eq!(
        built_lines(vec![listed(), file(), other]),
        built_lines(vec![listed(), file()])
    );
}
