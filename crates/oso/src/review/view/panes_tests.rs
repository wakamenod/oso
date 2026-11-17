//! The two panes on the view thread: a file's sides in step with filler rows, cards in their
//! side's pane, every row of a pane also in the unified text, and the panes' texts built only
//! while asked for and changed.
//!
//! Must not depend on: `app`, `dispatch` or winit; these lay out readings and comments made here.

use oso_core::review::{Anchor, Author, Review, RowKind, Rows, Side};

use super::*;
use crate::review::view::cards::tests::{read, snapshot};
use crate::review::view::section::tests::{hunk, lines_of, text_reading};
use crate::review::view::section::{self as unified, Shown};
use crate::review::view::{Tell, built};

const PATH: &[u8] = b"a.rs";

fn shown(cards: &Cards) -> Shown<'_> {
    Shown {
        cards,
        placed: None,
        cut_short: false,
        folded: false,
    }
}

fn line(side: Side, number: u32) -> Anchor {
    Anchor::Line { side, number }
}

/// A model of `PATH` changing old lines 2 and 3 into new lines 2 to 4, with a comment on old
/// line 2, a two-line one on new line 2, and one on the hunk.
fn model() -> (Review, crate::review::Reading) {
    let mut review = Review::new();
    let reading = read(
        &mut review,
        PATH,
        vec![hunk(
            2,
            2,
            &[b"old two\n", b"old three\n"],
            &[b"new two\n", b"new three\n", b"new four\n"],
        )],
    );
    for (anchor, text) in [
        (line(Side::Old, 2), "on old two"),
        (line(Side::New, 2), "on new two\nand more"),
        (Anchor::Hunk { index: 0 }, "on the hunk"),
    ] {
        review
            .add_comment(Author::Person, PATH, anchor, text.to_owned())
            .unwrap();
    }
    (review, reading)
}

/// Each row of both panes side by side: what it is and its text, the old pane's at the left.
fn side_by_side(pair: &Sides) -> String {
    let (old, new) = (lines_of(&pair.old), lines_of(&pair.new));
    assert_eq!(old.len(), new.len(), "the panes have as many rows");
    let name = |kind: RowKind| match kind {
        RowKind::File => "file",
        RowKind::Note => "note",
        RowKind::Hunk { .. } => "hunk",
        RowKind::Lines { .. } => "line",
        RowKind::Label { .. } => "label",
        RowKind::Card { .. } => "card",
        RowKind::Filler => "filler",
    };
    old.iter()
        .zip(&new)
        .map(|((old, at_old), (new, at_new))| {
            format!(
                "{:<6} {old:<22}| {:<6} {new}\n",
                name(*at_old),
                name(*at_new)
            )
        })
        .collect()
}

#[test]
fn each_side_has_its_pane_in_step_and_its_cards_under_its_lines() {
    let (review, reading) = model();
    let cards = snapshot(&review, PATH);
    let pair = render(&reading, usize::MAX, &shown(&cards));
    let expected = "\
file   modified  a.rs  +3 −2  · 3 comments| file   modified  a.rs  +3 −2  · 3 comments
hunk   @@ -2,2 +2,3 @@       | hunk   @@ -2,2 +2,3 @@
line   -old two              | line   +new two
label      ▍You  #0          | label      ▍You  #1
card       ▍  on old two     | card       ▍  on new two
filler                       | card       ▍  and more
line   -old three            | line   +new three
filler                       | line   +new four
filler                       | label      ▍You  #2
filler                       | card       ▍  on the hunk
";
    assert_eq!(side_by_side(&pair), expected);
}

#[test]
fn a_panes_lines_after_a_card_or_a_filler_keep_their_own_numbers() {
    let (review, reading) = model();
    let cards = snapshot(&review, PATH);
    let pair = render(&reading, usize::MAX, &shown(&cards));
    let numbers = |rendered: &section::Rendered| -> Vec<(Side, u32)> {
        let rows = Rows::new(vec![Arc::clone(&rendered.section)]);
        (0..rows.lines())
            .filter_map(|line| rows.at(line)?.line())
            .collect()
    };
    assert_eq!(numbers(&pair.old), [(Side::Old, 2), (Side::Old, 3)]);
    assert_eq!(
        numbers(&pair.new),
        [(Side::New, 2), (Side::New, 3), (Side::New, 4)]
    );
}

#[test]
fn every_row_of_a_pane_but_a_filler_is_in_the_unified_text() {
    let (review, reading) = model();
    let cards = snapshot(&review, PATH);
    let pair = render(&reading, usize::MAX, &shown(&cards));
    let whole = unified::render(&reading, usize::MAX, &shown(&cards));
    let rows_of = |rendered: &section::Rendered| {
        let rows = Rows::new(vec![Arc::clone(&rendered.section)]);
        (0..rows.lines())
            .map(|line| {
                let row = rows.at(line).unwrap();
                (row.kind, row.line())
            })
            .collect::<Vec<_>>()
    };
    let unified_rows = rows_of(&whole);
    let found = |(kind, line): &(RowKind, Option<(Side, u32)>)| match line {
        Some(line) => unified_rows
            .iter()
            .any(|(_, other)| other.as_ref() == Some(line)),
        None => unified_rows.iter().any(|(other, _)| other == kind),
    };
    for pane in [&pair.old, &pair.new] {
        for row in rows_of(pane) {
            assert!(row.0 == RowKind::Filler || found(&row), "{row:?}");
        }
    }
}

#[test]
fn a_reading_with_no_text_shows_its_note_in_both_and_its_cards_in_the_new() {
    let mut review = Review::new();
    let mut reading = text_reading(PATH, vec![hunk(1, 1, &[b"a\n"], &[b"b\n"])]);
    read(&mut review, PATH, vec![hunk(1, 1, &[b"a\n"], &[b"b\n"])]);
    review
        .add_comment(Author::Person, PATH, line(Side::New, 1), "kept".to_owned())
        .unwrap();
    reading.content = crate::review::Content::Binary { old: 1, new: 2 };
    let cards = snapshot(&review, PATH);
    let pair = render(&reading, usize::MAX, &shown(&cards));
    let text = side_by_side(&pair);
    assert!(text.contains("note   binary: 1 B → 2 B"), "{text}");
    assert!(
        text.contains("filler                       | label"),
        "{text}"
    );
}

#[test]
fn the_panes_texts_come_only_while_asked_for_and_changed() {
    let (_, reading) = model();
    let file = || Tell::File {
        reading: Arc::new(text_reading(PATH, vec![hunk(1, 1, &[b"a\n"], &[b"b\n"])])),
        carry: super::super::Carrying::Nothing,
    };
    let _ = reading;
    assert!(built("t", vec![file()]).panes.is_none());
    let text = built("t", vec![file(), Tell::Panes(true)]);
    let panes = text.panes.expect("asked for");
    let [old, new] = *panes;
    assert_eq!(old.1.lines(), new.1.lines());
    assert!(old.1.lines() > 2);

    let mut state = State::new("t".to_owned());
    state.take(file());
    state.take(Tell::Panes(true));
    assert!(state.build().panes.is_some());
    assert!(state.build().panes.is_none(), "nothing changed");
    state.take(Tell::Panes(false));
    state.take(Tell::Panes(true));
    assert!(state.build().panes.is_some(), "asked for again");
}

#[test]
fn a_file_that_may_hold_secrets_shows_only_its_count_in_both_panes() {
    let (review, mut reading) = model();
    reading.content = crate::review::Content::Withheld;
    let cards = snapshot(&review, PATH);
    for room in [usize::MAX, 0] {
        let pair = render(&reading, room, &shown(&cards));
        for pane in [&pair.old, &pair.new] {
            let kinds: Vec<RowKind> = lines_of(pane).into_iter().map(|(_, kind)| kind).collect();
            assert_eq!(kinds, [RowKind::File, RowKind::Note], "room {room}");
        }
    }
    let pair = gone(PATH, &cards, usize::MAX, false, true);
    for pane in [&pair.old, &pair.new] {
        assert_eq!(lines_of(pane).len(), 1, "the header alone");
    }
}

#[test]
fn past_the_limit_both_panes_show_the_same_files_header_alone() {
    let file = |path: &[u8]| Tell::File {
        reading: Arc::new(text_reading(
            path,
            vec![hunk(1, 1, &[b"a\n"], &[b"b\n", b"c\n"])],
        )),
        carry: super::super::Carrying::Nothing,
    };
    let listed = Tell::Listed {
        entries: [b"a.rs".as_slice(), b"b.rs"]
            .iter()
            .map(|path| text_reading(path, Vec::new()).entry.clone())
            .collect::<Vec<_>>()
            .into(),
        more: 0,
        cut: false,
    };
    let mut state = State::new("t".to_owned());
    state.take(listed);
    state.take(file(b"a.rs"));
    state.take(file(b"b.rs"));
    state.take(Tell::Panes(true));
    // Room for the title and a.rs's panes, counted together, but not for b.rs's.
    let whole = state.build();
    let [old, new] = *whole.panes.unwrap();
    let a = old.1.lines();
    assert_eq!(a, new.1.lines());
    state.max_text = 90;
    state.dirty = true;
    let cut = state.build();
    let [old, new] = *cut.panes.unwrap();
    assert_eq!(old.1.lines(), new.1.lines());
    assert!(old.1.lines() < a, "a file past the limit shows less");
    let notes = |rows: &Rows| {
        (0..rows.lines())
            .filter(|line| rows.at(*line).unwrap().kind == RowKind::Note)
            .count()
    };
    assert_eq!(notes(&old.1), notes(&new.1), "cut at the same file");
}
