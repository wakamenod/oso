//! The tests of sending a review's comments in the app: which comments a message gathers and
//! in what order, and which of those it took count as unchanged when it is sent.
//!
//! Must not depend on: key presses, how a frame is drawn, git or the threads.

use std::sync::Arc;

use oso_core::review::{Anchor, Author, Base, CommentId, Hunk, Review, RowKind, Rows, Section};
use oso_core::review::{Shape, Side};

use super::{places, same_text, unsent};

const A: &[u8] = b"a.rs";
const B: &[u8] = b"b.rs";

/// A review of `paths`, each one hunk adding three lines at line 1.
fn review_of(paths: &[&[u8]]) -> Review {
    let mut review = Review::new();
    for path in paths {
        let hunk = Hunk {
            old_start: 1,
            new_start: 1,
            removed: Vec::new(),
            added: vec![
                b"x".as_slice().into(),
                b"y".as_slice().into(),
                b"z".as_slice().into(),
            ],
        };
        let shape = Arc::new(Shape::new(Base::Unknown, vec![hunk]).unwrap());
        let carried = review
            .carry(path, shape)
            .run(|_, _| None, |_, _| Vec::new());
        review.apply(carried);
    }
    review
}

fn add(review: &mut Review, by: Author, path: &[u8], number: u32) -> CommentId {
    let anchor = Anchor::Line {
        side: Side::New,
        number,
    };
    review
        .add_comment(by, path, anchor, format!("on {number}"))
        .unwrap()
}

/// Rows that show the labels of `shown`, in that order, each in its file's section, with a
/// line of text between.
fn rows_showing(review: &Review, shown: &[CommentId]) -> Rows {
    let mut sections: Vec<Section> = Vec::new();
    for id in shown {
        let path = review.comment(*id).unwrap().path();
        if sections.last().is_none_or(|section| section.path() != path) {
            let mut section = Section::new(path.into());
            section.push(RowKind::File, 1);
            sections.push(section);
        }
        let section = sections.last_mut().unwrap();
        section.push(RowKind::Label { comment: *id }, 1);
        section.push(RowKind::Card { comment: *id }, 1);
    }
    Rows::new(sections.into_iter().map(Arc::new).collect())
}

#[test]
fn the_comments_go_in_the_order_the_review_shows_them_then_the_rest() {
    let mut review = review_of(&[A, B]);
    let first = add(&mut review, Author::Person, A, 3);
    let second = add(&mut review, Author::Person, B, 1);
    let third = add(&mut review, Author::Person, A, 1);
    let hidden = add(&mut review, Author::Person, B, 2);
    // The buffer shows b.rs's first, then a.rs's line 1 before its line 3.
    let rows = rows_showing(&review, &[second, third, first]);
    assert_eq!(
        unsent(&review, Some(&rows)),
        vec![second, third, first, hidden]
    );
    assert_eq!(unsent(&review, None), vec![first, third, second, hidden]);
}

#[test]
fn only_the_persons_unsent_comments_outside_resolved_threads_go() {
    let mut review = review_of(&[A]);
    let agent = add(&mut review, Author::Agent, A, 1);
    let reply = review.reply(Author::Person, agent, "yes".into()).unwrap();
    let sent = add(&mut review, Author::Person, A, 2);
    review.mark_sent(&[sent]);
    let resolved = add(&mut review, Author::Person, A, 3);
    let under = review
        .reply(Author::Person, resolved, "and".into())
        .unwrap();
    review.resolve(Author::Person, resolved, true).unwrap();
    assert_eq!(unsent(&review, None), vec![reply]);
    // Opened again, its thread goes once more.
    review.resolve(Author::Person, resolved, false).unwrap();
    assert_eq!(unsent(&review, None), vec![reply, resolved, under]);
    // An edit makes a sent comment unsent again.
    review
        .edit_comment(Author::Person, sent, "again".into())
        .unwrap();
    assert!(unsent(&review, None).contains(&sent));
}

#[test]
fn a_comment_edited_since_it_was_taken_is_not_the_same() {
    let mut review = review_of(&[A]);
    let id = add(&mut review, Author::Person, A, 1);
    let taken = review.comment(id).unwrap().clone();
    assert!(same_text(review.comment(id).unwrap(), &taken));
    review
        .edit_comment(Author::Person, id, "other words".into())
        .unwrap();
    assert!(!same_text(review.comment(id).unwrap(), &taken));
    // Back to the words the message holds: what it sends says them.
    review
        .edit_comment(Author::Person, id, "on 1".into())
        .unwrap();
    assert!(same_text(review.comment(id).unwrap(), &taken));
}

#[test]
fn a_pull_request_s_comments_go_where_its_head_is_checked_out_first() {
    use std::path::{Path, PathBuf};

    use crate::review::pr::PrFound;

    let root = Path::new("/repo");
    let found = |worktree: Option<&str>| PrFound {
        number: 7,
        title: String::new(),
        head_branch: "feature".into(),
        worktree: worktree.map(PathBuf::from),
        head_checked_out: false,
    };
    let looked = |found: Option<&PrFound>| -> Vec<PathBuf> {
        places(found, root).map(Path::to_path_buf).collect()
    };
    assert_eq!(looked(None), [PathBuf::from("/repo")]);
    assert_eq!(looked(Some(&found(None))), [PathBuf::from("/repo")]);
    assert_eq!(
        looked(Some(&found(Some("/trees/feature")))),
        [PathBuf::from("/trees/feature"), PathBuf::from("/repo")]
    );
}
