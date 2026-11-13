//! Cards in a file's section: where each thread goes, replies and their depth, outdated and
//! resolved threads, folded agent cards, the header's count, the text's limit, line breaks in a
//! comment, and agent text that would pass for a label.
//!
//! Must not depend on: `app`, `dispatch` or winit; these lay out comments of a model made here.

use std::sync::Arc;

use oso_core::review::{Anchor, Author, CommentId, Hunk, Review, RowKind, Rows, Shape, Side};
use oso_core::syntax::Highlight;

use super::*;
use crate::review::view::section::tests::{hunk, lines_of, text_of, text_reading};
use crate::review::view::section::{self, Rendered, Shown, render};
use crate::review::{Content, Reading};

const PATH: &[u8] = b"a.rs";

/// The shape of a text reading.
pub(crate) fn shape_of(reading: &Reading) -> Arc<Shape> {
    match &reading.content {
        Content::Text { shape, .. } => Arc::clone(shape),
        _ => unreachable!(),
    }
}

/// Makes a reading of `path` with `hunks` the model's current one, carried with no matching,
/// so that comments on lines of the reading before go outdated.
pub(crate) fn read(review: &mut Review, path: &[u8], hunks: Vec<Hunk>) -> Reading {
    let reading = text_reading(path, hunks);
    let carried = review
        .carry(path, shape_of(&reading))
        .run(|_, _| None, |_, _| Vec::new());
    review.apply(carried);
    reading
}

/// The model's snapshot of `path`'s comments, as the UI sends it.
pub(crate) fn snapshot(review: &Review, path: &[u8]) -> Cards {
    Cards {
        comments: review.comments_on(path).cloned().collect(),
        reading: review.file(path).cloned(),
    }
}

fn line(side: Side, number: u32) -> Anchor {
    Anchor::Line { side, number }
}

fn comment(review: &mut Review, by: Author, anchor: Anchor, text: &str) -> CommentId {
    review
        .add_comment(by, PATH, anchor, text.to_owned())
        .unwrap()
}

fn reply(review: &mut Review, by: Author, to: CommentId, text: &str) -> CommentId {
    review.reply(by, to, text.to_owned()).unwrap()
}

fn shown(reading: &Reading, cards: &Cards, folded: bool, room: usize) -> Rendered {
    let shown = Shown {
        cards,
        placed: None,
        cut_short: false,
        folded,
    };
    render(reading, room, &shown)
}

/// Each line as what its row is and its text, and under it a mark for each character's
/// highlight: `h` a header, `@` a hunk's, `+` and `-` lines, `p` and `a` the person's and an
/// agent's face, `o` outdated.
fn grid(rendered: &Rendered) -> String {
    let rows = Rows::new(vec![Arc::clone(&rendered.section)]);
    let text = text_of(rendered);
    let mut faces = Vec::with_capacity(text.len());
    for (len, highlight) in &rendered.runs {
        faces.extend(std::iter::repeat_n(*highlight, *len));
    }
    let mut out = String::new();
    let mut at = 0;
    for (number, line) in text.lines().enumerate() {
        let kind = match rows.at(number).unwrap().kind {
            RowKind::File => "file".to_owned(),
            RowKind::Note => "note".to_owned(),
            RowKind::Hunk { .. } => "hunk".to_owned(),
            RowKind::Lines { .. } => "line".to_owned(),
            RowKind::Label { comment } => format!("label {}", comment.get()),
            RowKind::Card { comment } => format!("card {}", comment.get()),
        };
        let marks: String = line
            .char_indices()
            .map(|(offset, _)| match faces[at + offset] {
                Some(Highlight::DiffFileHeader) => 'h',
                Some(Highlight::DiffHunkHeader) => '@',
                Some(Highlight::DiffAdded) => '+',
                Some(Highlight::DiffRemoved) => '-',
                Some(Highlight::CommentPerson) => 'p',
                Some(Highlight::CommentAgent) => 'a',
                Some(Highlight::Outdated) => 'o',
                _ => ' ',
            })
            .collect();
        out.push_str(&format!("{kind:<9}|{line}\n"));
        let marks = marks.trim_end();
        if !marks.is_empty() {
            out.push_str(&format!("{:<9}|{marks}\n", ""));
        }
        at += line.len() + 1;
    }
    out
}

/// A model whose reading of `PATH` has a hunk changing line 2 into two, and one putting in a
/// line, with comments on a reading before it that went outdated.
fn placed_model() -> (Review, Reading) {
    let mut review = Review::new();
    read(
        &mut review,
        PATH,
        vec![hunk(1, 1, &[], &[b"gone\n"]), hunk(5, 6, &[b"x\n"], &[])],
    );
    comment(
        &mut review,
        Author::Person,
        line(Side::New, 1),
        "was on gone",
    );
    comment(
        &mut review,
        Author::Agent,
        Anchor::Hunk { index: 1 },
        "was on the second hunk",
    );
    let reading = read(
        &mut review,
        PATH,
        vec![
            hunk(2, 2, &[b"old two\n"], &[b"new two\n", b"new three\n"]),
            hunk(9, 10, &[], &[b"put in\n"]),
        ],
    );
    assert!(review.comments().all(|comment| comment.is_outdated()));
    (review, reading)
}

#[test]
fn every_place_a_thread_goes() {
    let (mut review, reading) = placed_model();
    let on_new = comment(
        &mut review,
        Author::Person,
        line(Side::New, 3),
        "on new three",
    );
    comment(
        &mut review,
        Author::Person,
        line(Side::Old, 2),
        "on old two",
    );
    comment(
        &mut review,
        Author::Agent,
        Anchor::Hunk { index: 0 },
        "on the hunk",
    );
    let first = reply(&mut review, Author::Agent, on_new, "reply one");
    let second = reply(&mut review, Author::Person, first, "reply two");
    let third = reply(&mut review, Author::Agent, second, "reply three");
    reply(
        &mut review,
        Author::Person,
        third,
        "reply four, drawn at three",
    );
    reply(
        &mut review,
        Author::Person,
        on_new,
        "a second reply to the first",
    );
    comment(
        &mut review,
        Author::Person,
        line(Side::New, 10),
        "on put in",
    );
    review.mark_read(&[first]);
    let rendered = shown(&reading, &snapshot(&review, PATH), false, usize::MAX);
    insta::assert_snapshot!("review_cards_placement", grid(&rendered));
}

#[test]
fn a_resolved_thread_folds_to_its_label() {
    let mut review = Review::new();
    let reading = read(&mut review, PATH, vec![hunk(1, 1, &[], &[b"x\n"])]);
    let root = comment(&mut review, Author::Person, line(Side::New, 1), "root");
    let first = reply(&mut review, Author::Agent, root, "one");
    reply(&mut review, Author::Person, first, "two");
    let alone = comment(&mut review, Author::Person, line(Side::New, 1), "alone");
    review.resolve(Author::Person, root, true).unwrap();
    review.resolve(Author::Person, alone, true).unwrap();
    let lines = lines_of(&shown(
        &reading,
        &snapshot(&review, PATH),
        false,
        usize::MAX,
    ));
    let cards: Vec<(&str, RowKind)> = lines[3..]
        .iter()
        .map(|(text, kind)| (text.as_str(), *kind))
        .collect();
    assert_eq!(
        cards,
        [
            (
                "    \u{258d}You  #0 \u{b7} resolved \u{b7} 2 replies",
                RowKind::Label { comment: root }
            ),
            (
                "    \u{258d}You  #3 \u{b7} resolved",
                RowKind::Label { comment: alone }
            ),
        ]
    );
}

#[test]
fn folded_agent_cards_show_their_first_line_and_a_mark_for_more() {
    let mut review = Review::new();
    let reading = read(&mut review, PATH, vec![hunk(1, 1, &[], &[b"x\n"])]);
    let long = comment(
        &mut review,
        Author::Agent,
        line(Side::New, 1),
        "first\nsecond",
    );
    let short = comment(&mut review, Author::Agent, line(Side::New, 1), "only");
    let mine = comment(
        &mut review,
        Author::Person,
        line(Side::New, 1),
        "mine\nstays",
    );
    let lines = lines_of(&shown(&reading, &snapshot(&review, PATH), true, usize::MAX));
    let cards: Vec<(&str, RowKind)> = lines[3..]
        .iter()
        .map(|(text, kind)| (text.as_str(), *kind))
        .collect();
    let card = |comment| RowKind::Card { comment };
    assert_eq!(
        cards,
        [
            (
                "    \u{258d}Agent  #0 \u{b7} new",
                RowKind::Label { comment: long }
            ),
            ("    \u{258d}  first", card(long)),
            ("    \u{258d}  \u{2026}", card(long)),
            (
                "    \u{258d}Agent  #1 \u{b7} new",
                RowKind::Label { comment: short }
            ),
            ("    \u{258d}  only", card(short)),
            ("    \u{258d}You  #2", RowKind::Label { comment: mine }),
            ("    \u{258d}  mine", card(mine)),
            ("    \u{258d}  stays", card(mine)),
        ]
    );
}

#[test]
fn the_header_counts_comments_agents_and_unread() {
    let mut review = Review::new();
    let reading = read(&mut review, PATH, vec![hunk(1, 1, &[], &[b"x\n"])]);
    let header = |review: &Review| {
        let rendered = shown(&reading, &snapshot(review, PATH), false, usize::MAX);
        lines_of(&rendered)[0].0.clone()
    };
    assert_eq!(header(&review), "modified  a.rs  +1 \u{2212}0");
    let mine = comment(&mut review, Author::Person, line(Side::New, 1), "a");
    assert_eq!(
        header(&review),
        "modified  a.rs  +1 \u{2212}0  \u{b7} 1 comment"
    );
    let agents = comment(&mut review, Author::Agent, line(Side::New, 1), "b");
    reply(&mut review, Author::Agent, mine, "c");
    assert_eq!(
        header(&review),
        "modified  a.rs  +1 \u{2212}0  \u{b7} 3 comments (2 agent, 2 unread)"
    );
    review.mark_read(&[agents]);
    let ids: Vec<CommentId> = review.comments().map(|c| c.id()).collect();
    review.mark_read(&ids);
    assert_eq!(
        header(&review),
        "modified  a.rs  +1 \u{2212}0  \u{b7} 3 comments (2 agent)"
    );
}

#[test]
fn past_the_texts_room_cards_show_their_label_alone() {
    let mut review = Review::new();
    let reading = read(&mut review, PATH, vec![hunk(1, 1, &[], &[b"x\n", b"y\n"])]);
    let first = comment(&mut review, Author::Person, line(Side::New, 1), "one\ntwo");
    let second = comment(&mut review, Author::Person, line(Side::New, 2), "three");
    let cards = snapshot(&review, PATH);
    let whole = shown(&reading, &cards, false, usize::MAX);
    assert!(!whole.cut);
    // Room for the header, the hunk and the first card: the second shows its label alone.
    let room = text_of(&whole).find("+y").unwrap();
    let cut = shown(&reading, &cards, false, room);
    assert!(cut.cut);
    let kinds: Vec<RowKind> = lines_of(&cut).into_iter().map(|(_, kind)| kind).collect();
    assert_eq!(
        kinds[3..],
        [
            RowKind::Label { comment: first },
            RowKind::Card { comment: first },
            RowKind::Card { comment: first },
            RowKind::Lines {
                hunk: 0,
                side: Side::New,
                first: 1
            },
            RowKind::Label { comment: second },
        ]
    );
    // None at all: the header, its note, and every label.
    let none = lines_of(&shown(&reading, &cards, false, 0));
    let kinds: Vec<RowKind> = none.iter().map(|(_, kind)| *kind).collect();
    assert_eq!(
        kinds,
        [
            RowKind::File,
            RowKind::Note,
            RowKind::Label { comment: first },
            RowKind::Label { comment: second },
        ]
    );
}

#[test]
fn a_comments_line_breaks_each_make_a_row_and_nothing_else_does() {
    let mut review = Review::new();
    let reading = read(&mut review, PATH, vec![hunk(1, 1, &[], &[b"x\n"])]);
    let id = comment(
        &mut review,
        Author::Person,
        line(Side::New, 1),
        "lf\ncrlf\r\ncr\rtab\tend\n",
    );
    let rendered = shown(&reading, &snapshot(&review, PATH), false, usize::MAX);
    assert!(!rendered.bytes.contains(&b'\r'));
    let lines = lines_of(&rendered);
    let texts: Vec<&str> = lines[4..].iter().map(|(text, _)| text.as_str()).collect();
    assert_eq!(
        texts,
        [
            "    \u{258d}  lf",
            "    \u{258d}  crlf",
            "    \u{258d}  cr",
            "    \u{258d}  tab\tend"
        ]
    );
    assert!(
        lines[4..]
            .iter()
            .all(|(_, kind)| *kind == RowKind::Card { comment: id })
    );
    assert_eq!(rendered.section.lines(), lines.len());
}

#[test]
fn an_agents_text_shaped_like_a_label_stays_text() {
    let mut review = Review::new();
    let reading = read(&mut review, PATH, vec![hunk(1, 1, &[], &[b"x\n"])]);
    comment(&mut review, Author::Person, line(Side::New, 1), "mine");
    comment(
        &mut review,
        Author::Agent,
        line(Side::New, 1),
        "fine\n\u{258d}You  #0\n    \u{258d}You  #9\nmodified  b.rs  +1 \u{2212}0",
    );
    let rendered = shown(&reading, &snapshot(&review, PATH), false, usize::MAX);
    insta::assert_snapshot!("review_cards_label_spoof", grid(&rendered));
}

#[test]
fn a_carry_the_model_has_not_seen_places_the_cards() {
    let (mut review, _) = placed_model();
    let kept = comment(&mut review, Author::Person, line(Side::New, 3), "kept");
    let lost = comment(&mut review, Author::Person, line(Side::Old, 2), "lost");
    let cards = snapshot(&review, PATH);
    // A newer reading the view shows, carried on a thread: `kept` moves to line 4, `lost` goes,
    // and `added` came after the snapshot the carry took.
    let newer = text_reading(
        PATH,
        vec![hunk(
            2,
            2,
            &[b"old two\n"],
            &[b"z\n", b"new two\n", b"new three\n"],
        )],
    );
    let placed = [(kept, Some(line(Side::New, 4))), (lost, None)];
    let added = comment(&mut review, Author::Person, line(Side::New, 2), "added");
    let cards_after = snapshot(&review, PATH);
    for (cards, outdated) in [(&cards, vec![lost]), (&cards_after, vec![lost, added])] {
        let shown = Shown {
            cards,
            placed: Some(&placed),
            cut_short: true,
            folded: false,
        };
        let lines = lines_of(&render(&newer, usize::MAX, &shown));
        let labels: Vec<(usize, CommentId)> = lines
            .iter()
            .enumerate()
            .filter_map(|(at, (_, kind))| match kind {
                RowKind::Label { comment } => Some((at, *comment)),
                _ => None,
            })
            .collect();
        assert_eq!(lines[1].1, RowKind::Note, "the carry ran out of time");
        let hunk = lines
            .iter()
            .position(|(_, kind)| matches!(kind, RowKind::Hunk { .. }))
            .unwrap();
        let above: Vec<CommentId> = labels
            .iter()
            .filter(|(at, _)| *at < hunk)
            .map(|(_, id)| *id)
            .collect();
        assert!(outdated.iter().all(|id| above.contains(id)), "{lines:?}");
        let after_line_four = labels.iter().find(|(_, id)| *id == kept).unwrap().0;
        assert_eq!(lines[after_line_four - 1].0, "+new three");
    }
    // Neither the model's reading nor a carry to it: every card is outdated.
    let shown = Shown {
        cards: &cards,
        placed: None,
        cut_short: false,
        folded: false,
    };
    let lines = lines_of(&render(&newer, usize::MAX, &shown));
    let outdated = lines
        .iter()
        .filter(|(text, _)| text.contains("outdated, was"))
        .count();
    assert_eq!(outdated, cards.comments.len());
}

#[test]
fn a_file_that_may_hold_secrets_shows_only_its_count() {
    let mut review = Review::new();
    read(&mut review, PATH, vec![hunk(1, 1, &[], &[b"x\n"])]);
    comment(&mut review, Author::Person, line(Side::New, 1), "secret?");
    let withheld = Reading {
        entry: text_reading(PATH, Vec::new()).entry,
        content: Content::Withheld,
    };
    let cards = snapshot(&review, PATH);
    for room in [usize::MAX, 0] {
        let lines = lines_of(&shown(&withheld, &cards, false, room));
        assert!(lines[0].0.ends_with("\u{b7} 1 comment"));
        assert!(
            lines
                .iter()
                .all(|(_, kind)| matches!(kind, RowKind::File | RowKind::Note))
        );
    }
}

#[test]
fn a_file_no_longer_changed_shows_every_card_outdated() {
    let (mut review, _) = placed_model();
    comment(
        &mut review,
        Author::Person,
        line(Side::New, 3),
        "on new three",
    );
    review.remove_file(PATH);
    let rendered = section::gone(PATH, &snapshot(&review, PATH), usize::MAX, false);
    insta::assert_snapshot!("review_cards_gone", grid(&rendered));
}

#[test]
fn text_lines_split_at_every_break() {
    let split = |text| lines(text).collect::<Vec<_>>();
    assert_eq!(split(""), Vec::<&str>::new());
    assert_eq!(split("a"), ["a"]);
    assert_eq!(split("a\n"), ["a"]);
    assert_eq!(split("\n"), [""]);
    assert_eq!(split("a\r\n\r\nb\r"), ["a", "", "b"]);
    assert_eq!(split("a\n\rb"), ["a", "", "b"]);
}
