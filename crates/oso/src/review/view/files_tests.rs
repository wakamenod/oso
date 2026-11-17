//! The tests of a review's files pane's text: built only while asked for, a title and a row a
//! file shown, each with its marks, the filter's hidden files left out, and long paths cut from
//! the left.
//!
//! Must not depend on: `app` or the editor.

use std::sync::Arc;

use oso_core::review::{Anchor, Author, Filter, Review, Side};

use super::cards::tests::{read, snapshot};
use super::files::cut_left;
use super::section::tests::{entry, hunk};
use super::*;
use crate::review::list::Status;

/// A view of `src/a.rs` (one line put in, one taken out, viewed, a comment of the person's and
/// an unread agent's) and `new.rs` (two lines put in), each read as modified.
fn view() -> State {
    let mut state = State::new("Review".into());
    let entries: Arc<[Entry]> = vec![
        entry(b"src/a.rs", Status::Modified),
        entry(b"new.rs", Status::Added),
    ]
    .into();
    state.take(Tell::Listed {
        entries,
        more: 0,
        cut: false,
    });
    let mut review = Review::new();
    let readings = [
        (&b"src/a.rs"[..], hunk(1, 1, &[b"old\n"], &[b"new\n"])),
        (b"new.rs", hunk(1, 1, &[], &[b"x\n", b"y\n"])),
    ];
    for (path, hunk) in readings {
        let reading = Arc::new(read(&mut review, path, vec![hunk]));
        if path == b"src/a.rs" {
            let line = Anchor::Line {
                side: Side::New,
                number: 1,
            };
            review
                .add_comment(Author::Person, path, line, "mine".into())
                .unwrap();
            review
                .add_comment(Author::Agent, path, line, "theirs".into())
                .unwrap();
            review.set_viewed(path, 0, true);
        }
        let cards = snapshot(&review, path);
        state.take(Tell::Cards {
            path: path.into(),
            comments: cards.comments,
            reading: cards.reading,
            marks: cards.marks,
        });
        let carry = match &reading.content {
            crate::review::Content::Text { shape, .. } => review.carry(path, Arc::clone(shape)),
            _ => unreachable!(),
        };
        state.take(Tell::File {
            reading,
            carry: Carrying::Here(carry),
        });
    }
    state.take(Tell::Read);
    state
}

fn lines(state: &State) -> Vec<String> {
    state.files_pane_text().lines().map(str::to_owned).collect()
}

#[test]
fn the_pane_is_built_only_while_asked_for_a_row_a_file() {
    let mut state = view();
    assert!(state.build().files.is_none(), "not asked for");
    state.take(Tell::Files(Some(40)));
    let built = state.build();
    let pane = built.files.expect("the pane");
    assert_eq!(
        lines(&state),
        [
            "2 files \u{b7} / to filter",
            "M +1 \u{2212}1 1\u{b7}1\u{2022} \u{2713}  src/a.rs",
            "M +2 \u{2212}0  new.rs",
        ]
    );
    // Each file a row of its own, found by its path.
    assert_eq!(pane.1.line_of(b"new.rs", None), Some(2));
    // Nothing changed: no text; a width changed: laid out again.
    assert!(state.build().files.is_none());
    state.take(Tell::Files(Some(30)));
    assert!(state.build().files.is_some());
    state.take(Tell::Files(None));
    assert!(state.build().files.is_none());
}

#[test]
fn the_filter_leaves_its_hidden_files_out_and_says_so() {
    let mut state = view();
    state.take(Tell::Files(Some(40)));
    state.take(Tell::Filter(Filter::new("new")));
    assert!(state.build().files.is_some());
    assert_eq!(
        lines(&state),
        [
            "1 of 2 files hidden by \"new\" \u{b7} / to change",
            "M +2 \u{2212}0  new.rs",
        ]
    );
}

#[test]
fn a_long_path_is_cut_from_the_left_and_keeps_its_name() {
    assert_eq!(cut_left(b"src/a.rs", 20), b"src/a.rs");
    assert_eq!(
        String::from_utf8(cut_left(b"crates/oso/src/review/view.rs", 12)).unwrap(),
        "\u{2026}iew/view.rs"
    );
    // An invalid byte counts as the 4 cells it is drawn in.
    assert_eq!(cut_left(b"dir/\xff.rs", 6), "\u{2026}.rs".as_bytes());
    // A wide character counts as 2.
    assert_eq!(
        String::from_utf8(cut_left("a/\u{65e5}\u{672c}.rs".as_bytes(), 6)).unwrap(),
        "\u{2026}\u{672c}.rs"
    );
}

#[test]
fn nothing_is_sent_while_no_row_nor_the_title_changed() {
    let mut state = view();
    state.take(Tell::Files(Some(40)));
    assert!(state.build().files.is_some());
    // The review's own title changed: the pane's did not.
    state.take(Tell::Said(vec!["one note".into()]));
    assert!(state.build().files.is_none());
    // Hidden and shown again: the pane takes its text whole.
    state.take(Tell::Files(None));
    state.take(Tell::Files(Some(40)));
    assert!(state.build().files.is_some());
}

#[test]
fn a_long_path_is_cut_reading_only_its_end() {
    let long = "d/".repeat(1 << 19);
    let cut = cut_left(format!("{long}name.rs").as_bytes(), 12);
    assert_eq!(String::from_utf8(cut).unwrap(), "\u{2026}d/d/name.rs");
    // A tab takes its cells, so a path of tabs is cut too.
    let tabs = cut_left(&[b'\t'; 4096], 12);
    assert!(tabs.len() <= 12, "{}", tabs.len());
    // A cluster stays whole: no mark is left without its base.
    let marked = cut_left("abc/e\u{301}e\u{301}.rs".as_bytes(), 6);
    assert_eq!(
        String::from_utf8(marked).unwrap(),
        "\u{2026}e\u{301}e\u{301}.rs"
    );
}
