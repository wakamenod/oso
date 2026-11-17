//! The tests of a review's filter on its view: hidden files are not laid out in the text nor in
//! the panes, the title says how many are hidden, a file's comments match it, and a file that
//! may hold secrets is matched by its path alone.
//!
//! Must not depend on: `app` or the editor.

use std::sync::Arc;

use oso_core::review::{Anchor, Author, Filter, Review, RowKind, Side};

use super::cards::tests::{read, snapshot};
use super::section::tests::{entry, hunk};
use super::*;
use crate::review::Content;
use crate::review::list::Status;

/// A view of `a.rs`, `b.rs` and `.env`, each read with one line put in, a comment saying
/// "todo" on `a.rs` and one saying "todo too" on `.env`, whose lines are withheld.
fn view() -> State {
    let mut state = State::new("Review \u{b7} uncommitted \u{b7} oso".into());
    let entries: Arc<[Entry]> = vec![
        entry(b"a.rs", Status::Modified),
        entry(b"b.rs", Status::Modified),
        entry(b".env", Status::Modified),
    ]
    .into();
    state.take(Tell::Listed {
        entries,
        more: 0,
        cut: false,
    });
    let mut review = Review::new();
    let on_one = Anchor::Line {
        side: Side::New,
        number: 1,
    };
    for path in [&b"a.rs"[..], b"b.rs", b".env"] {
        let reading = Arc::new(read(&mut review, path, vec![hunk(1, 1, &[], &[b"x\n"])]));
        if path != b"b.rs" {
            let text = if path == b"a.rs" { "todo" } else { "todo too" };
            review
                .add_comment(Author::Person, path, on_one, text.into())
                .unwrap();
        }
        let cards = snapshot(&review, path);
        state.take(Tell::Cards {
            path: path.into(),
            comments: cards.comments,
            reading: cards.reading,
            marks: cards.marks,
        });
        let reading = if path == b".env" {
            Arc::new(Reading {
                entry: entry(path, Status::Modified),
                content: Content::Withheld,
            })
        } else {
            reading
        };
        let carry = match &reading.content {
            Content::Text { shape, .. } => review.carry(path, Arc::clone(shape)),
            _ => review.carry(
                path,
                Arc::new(
                    oso_core::review::Shape::new(oso_core::review::Base::Unknown, Vec::new())
                        .unwrap(),
                ),
            ),
        };
        state.take(Tell::File {
            reading,
            carry: Carrying::Here(carry),
        });
    }
    state.take(Tell::Read);
    state
}

/// The files the rows of `text` show, in order.
fn files_of(rows: &oso_core::review::Rows) -> Vec<String> {
    (0..rows.lines())
        .filter_map(|line| rows.at(line))
        .filter(|row| row.kind == RowKind::File)
        .map(|row| String::from_utf8_lossy(row.path).into_owned())
        .collect()
}

#[test]
fn hidden_files_are_not_laid_out_and_the_title_counts_them() {
    let mut state = view();
    let all = state.build();
    assert_eq!(files_of(&all.rows), ["a.rs", "b.rs", ".env"]);
    state.take(Tell::Filter(Filter::new("b.rs")));
    let filtered = state.build();
    assert_eq!(files_of(&filtered.rows), ["b.rs"]);
    let text = state.text.to_string();
    assert!(
        text.contains("2 of 3 files hidden by \"b.rs\" \u{b7} / to change\n"),
        "{text}"
    );
    assert!(!text.contains("a.rs"));
    // The filter gone, every file again.
    state.take(Tell::Filter(None));
    assert_eq!(files_of(&state.build().rows), ["a.rs", "b.rs", ".env"]);
    assert!(!state.text.to_string().contains("hidden by"));
}

#[test]
fn a_comments_text_matches_but_not_that_of_a_file_that_may_hold_secrets() {
    let mut state = view();
    state.take(Tell::Filter(Filter::new("todo")));
    let filtered = state.build();
    assert_eq!(
        files_of(&filtered.rows),
        ["a.rs"],
        "not .env, by its comment"
    );
    state.take(Tell::Filter(Filter::new("env")));
    assert_eq!(files_of(&state.build().rows), [".env"], "by its path");
}

#[test]
fn the_panes_hide_what_the_text_hides() {
    let mut state = view();
    state.take(Tell::Panes(true));
    state.take(Tell::Filter(Filter::new("a.rs")));
    let built = state.build();
    let panes = built.panes.expect("the panes");
    for (_, rows, _) in panes.iter() {
        assert_eq!(files_of(rows), ["a.rs"]);
    }
}
