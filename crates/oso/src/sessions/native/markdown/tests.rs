//! A reply's runs on its row: which replies are highlighted, each line's mark in its own face
//! and its text's runs past it, against what `render` draws.
//!
//! Must not depend on: the syntax threads; highlights are made by hand.

use oso_core::syntax::{Highlight, Highlights};
use proptest::prelude::*;

use super::{MAX_REPLY, MAX_RUNS, highlightable, plain, project};
use crate::sessions::native::render::tail;
use crate::sessions::native::rows::{Kind, Row};
use crate::sessions::native::shown::{LineStart, walk};

fn reply(text: &str, depth: u8) -> Row {
    Row {
        kind: Kind::Reply {
            text: text.to_owned(),
            streaming: false,
            cut: false,
        },
        depth,
        folded: false,
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

/// Checks `runs` over `row` as `render` draws it: each line's mark and each piece that has one
/// in `render`'s face, and the rest as the byte of the reply it shows, a fence's code too
/// unless none of it has a highlight.
fn check(row: &Row, highlights: &Highlights, runs: &[(usize, Option<Highlight>)]) {
    let Kind::Reply { text, .. } = &row.kind else {
        unreachable!()
    };
    let drawn = tail(row, LineStart::default());
    let total: usize = runs.iter().map(|(len, _)| len).sum();
    assert_eq!(total, drawn.text.len(), "the runs cover the row");
    let on_row = laid(total, runs);
    let drawn_runs = laid(total, &drawn.runs());
    let mut at = 0;
    let mut lines = drawn.text.split_inclusive('\n');
    walk(text, LineStart::default(), false, |pieces| {
        let line = lines.next().expect("a line drawn");
        let shown: usize = pieces.iter().map(|piece| piece.text.len()).sum();
        let mark = line.len() - 1 - shown;
        for byte in at..at + mark {
            assert_eq!(on_row.at(byte), drawn_runs.at(byte), "the mark's face");
        }
        let mut byte = at + mark;
        for piece in pieces {
            for within in 0..piece.text.len() {
                let found = on_row.at(byte + within);
                let face = drawn_runs.at(byte + within);
                let parsed = piece.from.and_then(|from| highlights.at(from + within));
                if piece.code.is_some() {
                    assert!(found == parsed || (parsed.is_none() && found == face));
                } else {
                    assert_eq!(found, face.or(parsed));
                }
            }
            byte += piece.text.len();
        }
        assert_eq!(on_row.at(byte), None, "a break has no run");
        at += line.len();
    });
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
fn a_fence_s_code_is_its_language_s_or_in_code_s_face() {
    let text = "```rust\nfn a\n```\n```zz\nb c\n```";
    let row = reply(text, 0);
    // `fn` is a keyword; the second fence's language gives nothing.
    let runs = project(&row, &marked(text, std::slice::from_ref(&(8..10)))).expect("projected");
    let drawn = tail(&row, LineStart::default()).text;
    assert_eq!(drawn, "● fn a\n\n  b c\n");
    let on_row = laid(drawn.len(), &runs);
    let at = |shown: &str| on_row.at(drawn.find(shown).expect("drawn"));
    assert_eq!(at("fn"), Some(Highlight::Keyword));
    assert_eq!(at("a\n"), None, "its language's plain text");
    assert_eq!(at("b c"), Some(Highlight::String), "code's face");
    // Not parsed yet, all of it is.
    let plain = laid(drawn.len(), &plain(&row).expect("a reply"));
    assert_eq!(plain.at(4), Some(Highlight::String));
}

#[test]
fn an_empty_reply_is_its_mark_alone() {
    let row = reply("", 0);
    let runs = project(&row, &Highlights::new(0)).expect("projected");
    check(&row, &Highlights::new(0), &runs);
}

#[test]
fn a_blank_line_has_no_mark_and_no_row_ends_in_a_space() {
    let text = "# A\n\n\r\nb";
    for depth in [0, 1] {
        let row = reply(text, depth);
        let drawn = tail(&row, LineStart::default()).text;
        assert!(drawn.lines().all(|line| !line.ends_with(' ')), "{drawn:?}");
        let highlights = marked(text, &[0..1, 1..3]);
        let runs = project(&row, &highlights).expect("projected");
        check(&row, &highlights, &runs);
        assert!(runs.iter().all(|(len, _)| *len > 0), "no empty run");
    }
    assert_eq!(
        tail(&reply(text, 1), LineStart::default()).text,
        "  ● A\n\n\n    b\n"
    );
    assert_eq!(tail(&reply("", 1), LineStart::default()).text, "  ●\n");
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
    for text in [
        "one\ntwo",
        "",
        "a\r\n\nb\n",
        "x\r",
        "# a `b`\n- c\n```\nd\n```",
        "```\n```",
    ] {
        for depth in 0..2 {
            let row = reply(text, depth);
            let drawn = tail(&row, LineStart::default());
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
    };
    assert_eq!(plain(&note), None);
}

proptest! {
    /// Whatever the text, its line breaks and its highlights, the runs lie where `render` puts
    /// each byte.
    #[test]
    fn the_runs_lie_where_render_puts_each_byte(
        text in "(\r|\n|\r\n|a|é|●|`|```|```rust\n| |# |- |\\*|> |---){0,40}",
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
