//! The tests of a review's comments in the app: which agent comments count as read from the
//! rows in view, and which files left a listing.
//!
//! Must not depend on: key presses, how a frame is drawn, git or the threads.

use std::sync::Arc;

use oso_core::review::{
    Anchor, Author, Base, CommentId, Hunk, Review, RowKind, Rows, Section, Shape, Side,
};

use super::{read_in_view, unlisted};
use crate::review::{Entry, Id, Status};

const FILE: &[u8] = b"a.rs";

fn entry(path: &[u8]) -> Entry {
    let zero = Id::parse(&[b'0'; 40]).unwrap();
    Entry {
        path: path.into(),
        old_path: None,
        status: Status::Modified,
        old_mode: 0o100_644,
        new_mode: 0o100_644,
        old_id: zero.clone(),
        new_id: zero,
    }
}

/// A review of `paths`, each one hunk adding two lines at line 1.
fn review_of(paths: &[&[u8]]) -> Review {
    let mut review = Review::new();
    for path in paths {
        let hunk = Hunk {
            old_start: 1,
            new_start: 1,
            removed: Vec::new(),
            added: vec![b"x".as_slice().into(), b"y".as_slice().into()],
        };
        let shape = Arc::new(Shape::new(Base::Unknown, vec![hunk]).unwrap());
        let carried = review
            .carry(path, shape)
            .run(|_, _| None, |_, _| Vec::new());
        review.apply(carried);
    }
    review
}

fn line(number: u32) -> Anchor {
    Anchor::Line {
        side: Side::New,
        number,
    }
}

/// The rows of [`FILE`]: a header, a hunk header, its first line, then `comment`'s label and
/// `text` rows of its text, then its second line.
fn rows_with(comment: CommentId, text: usize) -> Rows {
    let mut section = Section::new(FILE.into());
    section.push(RowKind::File, 1);
    section.push(RowKind::Hunk { hunk: 0 }, 1);
    let lines = |first| RowKind::Lines {
        hunk: 0,
        side: Side::New,
        first,
    };
    section.push(lines(1), 1);
    section.push(RowKind::Label { comment }, 1);
    section.push(RowKind::Card { comment }, text);
    section.push(lines(2), 1);
    Rows::new(vec![Arc::new(section)])
}

#[test]
fn an_agent_comment_is_read_only_once_its_label_and_whole_text_are_in_view() {
    let mut review = review_of(&[FILE]);
    let agent = review
        .add_comment(Author::Agent, FILE, line(1), "one\ntwo\nthree".into())
        .unwrap();
    let rows = rows_with(agent, 3);
    // Lines 3 (label) to 6 (last text row).
    assert_eq!(
        read_in_view(&rows, &review, 0, 5, false).0,
        Vec::<CommentId>::new()
    );
    assert_eq!(
        read_in_view(&rows, &review, 4, 7, false).0,
        Vec::<CommentId>::new()
    );
    let (read, paths) = read_in_view(&rows, &review, 2, 6, false);
    assert_eq!(read, [agent]);
    assert_eq!(paths, [Box::<[u8]>::from(FILE)]);
    review.mark_read(&read);
    assert!(
        read_in_view(&rows, &review, 0, 7, false).0.is_empty(),
        "read once"
    );
}

#[test]
fn an_agent_comment_folded_or_shown_as_its_label_alone_is_not_read() {
    let mut review = review_of(&[FILE]);
    let agent = review
        .add_comment(Author::Agent, FILE, line(1), "one\ntwo".into())
        .unwrap();
    // Folded to its first line and a mark for more.
    let rows = rows_with(agent, 2);
    assert!(read_in_view(&rows, &review, 0, 10, true).0.is_empty());
    // Its label alone, as past the text's room or in a resolved thread.
    let rows = rows_with(agent, 0);
    assert!(read_in_view(&rows, &review, 0, 10, false).0.is_empty());
    let rows = rows_with(agent, 2);
    assert_eq!(read_in_view(&rows, &review, 0, 10, false).0, [agent]);
}

#[test]
fn the_persons_comments_are_never_marked() {
    let mut review = review_of(&[FILE]);
    let mine = review
        .add_comment(Author::Person, FILE, line(1), "mine".into())
        .unwrap();
    let rows = rows_with(mine, 1);
    assert!(read_in_view(&rows, &review, 0, 10, false).0.is_empty());
}

#[test]
fn files_a_listing_no_longer_names_are_found() {
    let review = review_of(&[FILE, b"b.rs", b"c.rs"]);
    let entries = [entry(FILE), entry(b"c.rs")];
    assert_eq!(
        unlisted(&review, &entries),
        [Box::<[u8]>::from(b"b.rs".as_slice())]
    );
}
