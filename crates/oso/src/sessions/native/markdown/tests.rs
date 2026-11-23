//! A reply's runs on its row: which replies are highlighted, each line's mark in its own face
//! and its text's runs past it, against what `render` draws.
//!
//! Must not depend on: the syntax threads; highlights are made by hand.

use oso_core::syntax::{Highlight, Highlights};
use proptest::prelude::*;

use super::{MAX_REPLY, MAX_RUNS, highlightable, plain, project};
use crate::sessions::native::render::tail;
use crate::sessions::native::rows::{Kind, Row};

fn reply(text: &str, depth: u8) -> Row {
    Row {
        kind: Kind::Reply {
            text: text.to_owned(),
            streaming: false,
            cut: false,
        },
        depth,
        folded: false,
        gap: false,
    }
}

/// `text`'s highlights: `Keyword` on each byte range given, the rest plain.
fn marked(text: &str, ranges: &[std::ops::Range<usize>]) -> Highlights {
    let mut highlights = Highlights::new(text.len());
    for range in ranges {
        highlights.replace(range.clone(), [(range.len(), Some(Highlight::Keyword))]);
    }
    highlights
}

/// The highlights `runs` give over `len` bytes.
fn laid(len: usize, runs: &[(usize, Option<Highlight>)]) -> Highlights {
    let mut highlights = Highlights::new(len);
    highlights.replace(0..len, runs.iter().copied());
    highlights
}

/// Checks `runs` over `row` as `render` draws it: each line's mark as `render`'s face, and
/// each byte of a line's text as the byte of the reply it shows.
fn check(row: &Row, highlights: &Highlights, runs: &[(usize, Option<Highlight>)]) {
    let Kind::Reply { text, .. } = &row.kind else {
        unreachable!()
    };
    let drawn = tail(row, 0, 0);
    let total: usize = runs.iter().map(|(len, _)| len).sum();
    assert_eq!(total, drawn.text.len(), "the runs cover the row");
    let on_row = laid(total, runs);
    let marks = laid(total, &drawn.runs());
    let mut at = 0;
    // `str::lines`, as `render` draws them: each line's offset in the reply, found apart.
    let lines: Vec<&str> = text.lines().collect();
    for (index, line) in drawn.text.split_inclusive('\n').enumerate() {
        let Some(shown) = lines.get(index) else {
            panic!("a line the reply has not: {line:?}");
        };
        let mark = line.len() - shown.len() - 1;
        assert_eq!(&line[mark..line.len() - 1], *shown);
        for byte in 0..mark {
            assert_eq!(on_row.at(at + byte), marks.at(at + byte), "the mark's face");
        }
        let from = shown.as_ptr() as usize - text.as_ptr() as usize;
        for byte in 0..shown.len() {
            assert_eq!(on_row.at(at + mark + byte), highlights.at(from + byte));
        }
        assert_eq!(on_row.at(at + line.len() - 1), None, "a break has no run");
        at += line.len();
    }
}

#[test]
fn each_line_s_runs_go_past_its_mark() {
    let text = "# Title\nSome `code` here\r\n\n```rust\nfn a() {}\n```";
    let row = reply(text, 0);
    let highlights = marked(text, &[0..7, 13..19, 27..44]);
    let runs = project(&row, &highlights).expect("projected");
    check(&row, &highlights, &runs);
    // A subagent's reply is indented, and so are its marks.
    let row = reply(text, 1);
    check(
        &row,
        &highlights,
        &project(&row, &highlights).expect("projected"),
    );
}

#[test]
fn an_empty_reply_draws_nothing() {
    let row = reply("", 0);
    let runs = project(&row, &Highlights::new(0)).expect("projected");
    check(&row, &Highlights::new(0), &runs);
}

#[test]
fn a_blank_line_has_no_mark_and_no_row_ends_in_a_space() {
    let text = "# A\n\n\r\nb";
    for depth in [0, 1] {
        let row = reply(text, depth);
        let drawn = tail(&row, 0, 0).text;
        assert!(drawn.lines().all(|line| !line.ends_with(' ')), "{drawn:?}");
        let highlights = marked(text, &[0..1, 1..3]);
        let runs = project(&row, &highlights).expect("projected");
        check(&row, &highlights, &runs);
        assert!(runs.iter().all(|(len, _)| *len > 0), "no empty run");
    }
    assert_eq!(tail(&reply(text, 1), 0, 0).text, "│   # A\n│\n│\n│   b\n");
    assert_eq!(tail(&reply("", 1), 0, 0).text, "");
}

#[test]
fn only_a_whole_reply_within_the_limit_is_highlighted() {
    let streaming = Row {
        kind: Kind::Reply {
            text: "a".into(),
            streaming: true,
            cut: false,
        },
        depth: 0,
        folded: false,
        gap: false,
    };
    assert_eq!(highlightable(&streaming), None);
    let cut = Row {
        kind: Kind::Reply {
            text: "a".into(),
            streaming: false,
            cut: true,
        },
        ..streaming.clone()
    };
    assert_eq!(highlightable(&cut), None);
    let note = Row {
        kind: Kind::Note("a".into()),
        ..streaming
    };
    assert_eq!(highlightable(&note), None);
    let at_limit = "a".repeat(MAX_REPLY);
    assert!(highlightable(&reply(&at_limit, 0)).is_some());
    let past = "a".repeat(MAX_REPLY + 1);
    assert_eq!(highlightable(&reply(&past, 0)), None);
    assert_eq!(
        project(&reply(&past, 0), &Highlights::new(past.len())),
        None
    );
}

#[test]
fn highlights_of_another_text_or_too_many_runs_leave_it_plain() {
    let row = reply("abc", 0);
    assert_eq!(project(&row, &Highlights::new(2)), None, "another length");
    // Every other byte highlighted, on one line within the byte limit: past the runs' limit.
    let text = "ab".repeat(MAX_REPLY / 2);
    let ranges: Vec<_> = (0..MAX_REPLY / 2).map(|at| 2 * at..2 * at + 1).collect();
    let highlights = marked(&text, &ranges);
    assert!(highlightable(&reply(&text, 0)).is_some());
    assert!(2 * ranges.len() >= MAX_RUNS);
    assert_eq!(project(&reply(&text, 0), &highlights), None);
    // A quarter as many fit.
    let text = "ab".repeat(MAX_REPLY / 8);
    let ranges: Vec<_> = (0..MAX_REPLY / 8).map(|at| 2 * at..2 * at + 1).collect();
    let highlights = marked(&text, &ranges);
    assert!(project(&reply(&text, 0), &highlights).is_some());
}

#[test]
fn plain_is_what_the_row_draws() {
    for text in ["one\ntwo", "", "a\r\n\nb\n", "x\r"] {
        for depth in 0..2 {
            let row = Row {
                gap: depth == 1,
                ..reply(text, depth)
            };
            let drawn = tail(&row, 0, 0);
            let runs = plain(&row).expect("a reply");
            let total: usize = runs.iter().map(|(len, _)| len).sum();
            assert_eq!(total, drawn.text.len(), "{text:?}");
            let (plain, faces) = (laid(total, &runs), laid(total, &drawn.runs()));
            assert!(
                plain.runs(0..total).eq(faces.runs(0..total)),
                "{text:?} at depth {depth}"
            );
        }
    }
    let note = Row {
        kind: Kind::Note("a".into()),
        depth: 0,
        folded: false,
        gap: false,
    };
    assert_eq!(plain(&note), None);
}

proptest! {
    /// Whatever the text, its line breaks and its highlights, the runs lie where `render` puts
    /// each byte.
    #[test]
    fn the_runs_lie_where_render_puts_each_byte(
        text in "(\r|\n|\r\n|a|é|●|`| ){0,40}",
        depth in 0u8..2,
        cuts in prop::collection::vec(any::<prop::sample::Index>(), 0..8),
    ) {
        let mut at: Vec<usize> = cuts
            .iter()
            .map(|cut| text.floor_char_boundary(cut.index(text.len() + 1)))
            .collect();
        at.sort_unstable();
        let ranges: Vec<_> = at.chunks(2).filter(|pair| pair.len() == 2).map(|pair| pair[0]..pair[1]).collect();
        let highlights = marked(&text, &ranges);
        let row = reply(&text, depth);
        let runs = project(&row, &highlights).expect("projected");
        check(&row, &highlights, &runs);
    }
}

#[test]
fn many_short_lines_count_their_marks_and_breaks_as_runs() {
    // Each line is a mark, a byte and a break: a third of the runs' limit in lines passes it.
    let text = "a\n".repeat(MAX_RUNS / 3 + 1);
    assert!(text.len() <= MAX_REPLY);
    assert_eq!(
        project(&reply(&text, 0), &Highlights::new(text.len())),
        None
    );
    let text = "a\n".repeat(MAX_RUNS / 4);
    assert!(project(&reply(&text, 0), &Highlights::new(text.len())).is_some());
}
