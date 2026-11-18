//! A file's section as text and rows: headers, hunks, every note, line endings, and paths or
//! text that would pass for a row of another kind.
//!
//! Must not depend on: `app`, `dispatch` or winit; these lay out readings made here.

use std::sync::Arc;
use std::time::{Duration, Instant};

use oso_core::review::{Base, Hunk, RowKind, Rows, Shape, Side};
use oso_core::syntax::Highlight;
use proptest::prelude::*;

use super::*;
use crate::git::Id;
use crate::review::{Content, Entry, Reading};

pub(crate) fn entry(path: &[u8], status: Status) -> Entry {
    let zero = Id::parse(&[b'0'; 40]).unwrap();
    Entry {
        path: path.into(),
        old_path: None,
        status,
        old_mode: 0o100_644,
        new_mode: 0o100_644,
        old_id: zero.clone(),
        new_id: zero,
    }
}

pub(crate) fn hunk(old_start: u32, new_start: u32, removed: &[&[u8]], added: &[&[u8]]) -> Hunk {
    Hunk {
        old_start,
        new_start,
        removed: removed.iter().map(|line| (*line).into()).collect(),
        added: added.iter().map(|line| (*line).into()).collect(),
    }
}

pub(crate) fn text_reading(path: &[u8], hunks: Vec<Hunk>) -> Reading {
    Reading {
        entry: entry(path, Status::Modified),
        content: Content::Text {
            refined: Refined::of(&hunks, Instant::now() + Duration::from_secs(5)),
            shape: Arc::new(Shape::new(Base::Unknown, hunks).unwrap()),
            old: Arc::from(&b""[..]),
            new: Arc::from(&b""[..]),
        },
    }
}

fn reading(content: Content) -> Reading {
    Reading {
        entry: entry(b"src/a.rs", Status::Modified),
        content,
    }
}

/// The section of `reading` with no comments; `whole` false as past the text's limit.
fn plain(reading: &Reading, whole: bool) -> Rendered {
    let cards = Cards::default();
    let shown = Shown {
        cards: &cards,
        placed: None,
        cut_short: false,
        folded: false,
    };
    render(reading, if whole { usize::MAX } else { 0 }, &shown)
}

pub(crate) fn text_of(rendered: &Rendered) -> String {
    String::from_utf8_lossy(&rendered.bytes).into_owned()
}

/// Each line's text and what its row is.
pub(crate) fn lines_of(rendered: &Rendered) -> Vec<(String, RowKind)> {
    let rows = Rows::new(vec![Arc::clone(&rendered.section)]);
    let text = text_of(rendered);
    text.lines()
        .enumerate()
        .map(|(line, text)| (text.to_owned(), rows.at(line).unwrap().kind))
        .collect()
}

#[test]
fn a_text_reading_shows_its_header_hunks_and_lines() {
    let reading = text_reading(
        b"src/a.rs",
        vec![
            hunk(2, 2, &[b"old\n"], &[b"new\n", b"more\n"]),
            hunk(9, 10, &[], &[b"put in\n"]),
        ],
    );
    let rendered = plain(&reading, true);
    let lines = lines_of(&rendered);
    let hunk_lines = |hunk, side, first| RowKind::Lines { hunk, side, first };
    assert_eq!(
        lines,
        [
            ("modified  src/a.rs  +3 \u{2212}1".into(), RowKind::File),
            ("@@ -2 +2,2 @@".into(), RowKind::Hunk { hunk: 0 }),
            ("-old".into(), hunk_lines(0, Side::Old, 2)),
            ("+new".into(), hunk_lines(0, Side::New, 2)),
            ("+more".into(), hunk_lines(0, Side::New, 2)),
            ("@@ -8,0 +10 @@".into(), RowKind::Hunk { hunk: 1 }),
            ("+put in".into(), hunk_lines(1, Side::New, 10)),
        ]
    );
    let rows = Rows::new(vec![Arc::clone(&rendered.section)]);
    assert_eq!(rows.at(4).unwrap().line(), Some((Side::New, 3)));
    assert_eq!(rows.lines(), 7);
    let total: usize = rendered.runs.iter().map(|(len, _)| len).sum();
    assert_eq!(total, rendered.bytes.len());
    assert_eq!(
        rendered.runs[0],
        (lines[0].0.len(), Some(Highlight::DiffFileHeader))
    );
}

#[test]
fn a_changed_word_is_marked_after_the_sign_by_the_bytes_the_buffer_holds() {
    let reading = text_reading(
        b"src/a.rs",
        vec![
            hunk(1, 1, &[b"let a = 1;\n"], &[b"let a = 2;\n"]),
            // An invalid byte before the word is three bytes in the buffer, as U+FFFD.
            hunk(5, 5, &[b"x \xff y\n"], &[b"x \xff z\n"]),
            // Lines added alone are not refined.
            hunk(9, 9, &[], &[b"more\n"]),
        ],
    );
    let rendered = plain(&reading, true);
    let rows = Rows::new(vec![Arc::clone(&rendered.section)]);
    // The file's header, the hunk's, then `-let a = 1;` and `+let a = 2;`.
    assert_eq!(rows.refined(2), std::slice::from_ref(&(9..10)));
    assert_eq!(rows.refined(3), std::slice::from_ref(&(9..10)));
    assert_eq!(rows.refined(5), std::slice::from_ref(&(7..8)));
    assert_eq!(rows.refined(6), std::slice::from_ref(&(7..8)));
    assert!(rows.refined(8).is_empty());
    assert!(rows.refined(0).is_empty() && rows.refined(1).is_empty());
    // Each span counts against the review's text limit with the text.
    assert_eq!(rendered.marks, 4);
    assert_eq!(rendered.held(), rendered.bytes.len() + 4 * MARK_COST);
}

#[test]
fn every_note_kind_says_what_it_hides() {
    let cases = [
        (
            Content::Binary { old: 10, new: 2048 },
            "binary: 10 B \u{2192} 2.0 KiB",
        ),
        (
            Content::TooLarge {
                old: 9 << 20,
                new: 9 << 20,
            },
            "too large to show: 9.0 MiB \u{2192} 9.0 MiB",
        ),
        (Content::OutOfTime, "not read in time: g reads it again"),
        (Content::Withheld, "may hold secrets: not shown"),
        (Content::Conflicted, "conflicted: not shown"),
        (Content::Submodule, "submodule: none \u{2192} none"),
        (
            Content::Unreadable("no such file\nsecond".into()),
            "cannot be read: no such file second",
        ),
    ];
    for (content, note) in cases {
        let lines = lines_of(&plain(&reading(content), true));
        assert_eq!(lines.len(), 2, "{note}");
        assert_eq!(lines[0].1, RowKind::File);
        assert_eq!(lines[1], (note.to_owned(), RowKind::Note));
    }
    let lines = lines_of(&unread(&entry(b"b", Status::Added), &Cards::default()));
    assert_eq!(
        lines,
        [
            ("added  b".into(), RowKind::File),
            ("reading\u{2026}".into(), RowKind::Note)
        ]
    );
    let past = lines_of(&plain(
        &text_reading(b"c", vec![hunk(1, 1, &[b"a\n"], &[])]),
        false,
    ));
    assert_eq!(past[0].0, "modified  c  +0 \u{2212}1");
    assert_eq!(past[1].1, RowKind::Note);
    assert_eq!(past.len(), 2);
}

#[test]
fn line_endings_and_a_last_line_without_newline_are_noted() {
    let reading = text_reading(b"a", vec![hunk(1, 1, &[b"x\r\n"], &[b"x\n", b"end"])]);
    let lines = lines_of(&plain(&reading, true));
    let notes: Vec<&str> = lines
        .iter()
        .filter(|(_, kind)| *kind == RowKind::Note)
        .map(|(text, _)| text.as_str())
        .collect();
    assert_eq!(
        notes,
        [
            "line endings differ: CR LF on the old side",
            "no newline at the end of the new side"
        ]
    );
    // The `\r` is taken off, so the two lines read the same but for their sign.
    assert!(lines.iter().any(|(text, _)| text == "-x"));
    assert!(lines.iter().any(|(text, _)| text == "+end"));
}

#[test]
fn a_path_or_a_reason_never_makes_a_line_of_its_own() {
    let mut entry = entry(b"a\nmodified  b\xff\xe2\x80\xae.rs", Status::Renamed);
    entry.old_path = Some(b"old\npath".as_slice().into());
    let rendered = unread(&entry, &Cards::default());
    assert_eq!(rendered.bytes.iter().filter(|&&b| b == b'\n').count(), 2);
    assert_eq!(rendered.section.lines(), 2);
    let text = text_of(&rendered);
    assert!(text.starts_with("renamed  old\u{2424}path \u{2192} a\u{2424}modified  b"));
    // The invalid byte and the bidi override go in as they are, for the display map to show.
    assert!(rendered.bytes.windows(4).any(|w| w == b"\xff\xe2\x80\xae"));
    let total: usize = rendered.runs.iter().map(|(len, _)| len).sum();
    assert_eq!(total, decoded_len(&rendered.bytes));
    let said = title("Review\nuncommitted", &["first\nsecond".into()]);
    assert_eq!(said.section.lines(), 2);
    assert_eq!(text_of(&said), "Review uncommitted\nfirst second\n");
}

#[test]
fn a_lines_text_cannot_pass_for_a_header() {
    let reading = text_reading(
        b"a",
        vec![hunk(1, 1, &[], &[b"modified  evil  +1 \xe2\x88\x921\n"])],
    );
    let lines = lines_of(&plain(&reading, true));
    assert_eq!(lines.len(), 3);
    assert_eq!(
        lines[2].1,
        RowKind::Lines {
            hunk: 0,
            side: Side::New,
            first: 1
        }
    );
}

proptest! {
    #[test]
    fn decoded_len_is_the_loaders(bytes in proptest::collection::vec(any::<u8>(), 0..64)) {
        // A first byte that cannot start a BOM, which the loader would take off.
        let mut text = vec![b'x'];
        text.extend_from_slice(&bytes);
        text.push(b'\n');
        let loaded = oso_core::load::decode_all(&text, Default::default());
        prop_assert_eq!(decoded_len(&text), loaded.text().len());
    }
}

/// The file the decisions are on.
const PATH: &[u8] = b"a.rs";

#[test]
fn decisions_show_on_their_hunks_header_and_the_files() {
    let mut review = oso_core::review::Review::new();
    let reading = crate::review::view::cards::tests::read(
        &mut review,
        PATH,
        vec![
            hunk(2, 2, &[b"two\n"], &[b"2\n"]),
            hunk(6, 6, &[b"six\n"], &[b"6\n"]),
        ],
    );
    review.decide(PATH, 0, Some(oso_core::review::Decision::Accepted));
    review.set_viewed(PATH, 0, true);
    review.set_viewed(PATH, 1, true);
    let rendered = decided_shown(&reading, &review);
    let text = text_of(&rendered);
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[0].ends_with("  \u{b7} 1/2 decided"), "{text}");
    assert_eq!(lines[1], "@@ -2 +2 @@  \u{b7} accepted \u{b7} viewed");
    assert!(lines.contains(&"@@ -6 +6 @@  \u{b7} viewed"), "{text}");
    // Read again with the second hunk rewritten: the first keeps its marks, the second's drop
    // and are said; and the reading before, shown with this snapshot, marks nothing.
    let other = crate::review::view::cards::tests::read(
        &mut review,
        PATH,
        vec![
            hunk(2, 2, &[b"two\n"], &[b"2\n"]),
            hunk(6, 6, &[b"six\n"], &[b"VI\n"]),
        ],
    );
    let again = text_of(&decided_shown(&other, &review));
    assert!(
        again.contains("1 decision dropped: its hunk changed\n@@ -2 +2 @@  \u{b7} accepted"),
        "{again}"
    );
    let stale = text_of(&decided_shown(&reading, &review));
    assert!(
        !stale.contains("decided") && !stale.contains("accepted"),
        "{stale}"
    );
}

/// `reading` laid out with the model's comments and marks.
fn decided_shown(reading: &Reading, review: &oso_core::review::Review) -> Rendered {
    let cards = crate::review::view::cards::tests::snapshot(review, PATH);
    let shown = Shown {
        cards: &cards,
        placed: None,
        cut_short: false,
        folded: false,
    };
    render(reading, usize::MAX, &shown)
}
