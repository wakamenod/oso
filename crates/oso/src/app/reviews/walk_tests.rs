//! A pull request's walk through its commits (ADR 0167): where each step goes, the merges and
//! commits that change nothing stepped over, the edges, the reviews used last, and the marks
//! counted only from a model that changed.
//!
//! Must not depend on: the app's threads or windows; a walk is plain data here.

use oso_core::editor::Step;
use oso_core::review::{Anchor, Author, Base, Hunk, Review, Shape, Side};

use super::Walk;
use crate::git::Id;
use crate::review::commits::{Listed, Listing};

fn listing(n: u8, merge: bool, empty: bool) -> Listing {
    Listing {
        id: Id::parse(&[b'0' + n; 40]).unwrap(),
        short: format!("{n}{n}{n}{n}{n}{n}{n}"),
        subject: format!("Commit {n}"),
        author: "Ann".into(),
        date: "2026-11-20".into(),
        merge,
        empty,
        unpushed: false,
    }
}

/// Five commits: the third a merge, the fifth changing nothing.
fn walk() -> Walk {
    let commits = vec![
        listing(1, false, false),
        listing(2, false, false),
        listing(3, true, false),
        listing(4, false, false),
        listing(5, false, true),
    ];
    Walk::of(
        12,
        Listed {
            commits,
            more: false,
        },
    )
}

#[test]
fn next_goes_from_the_whole_to_the_first_and_steps_over_merges() {
    let walk = walk();
    assert_eq!(walk.target(None, Step::Next), Ok(Some(0)));
    assert_eq!(walk.target(Some(0), Step::Next), Ok(Some(1)));
    assert_eq!(
        walk.target(Some(1), Step::Next),
        Ok(Some(3)),
        "the merge stepped over"
    );
    // The last that changes something is the end.
    assert_eq!(
        walk.target(Some(3), Step::Next),
        Err("4444444 is the last commit of #12".into())
    );
}

#[test]
fn previous_goes_from_the_first_to_the_whole_and_from_the_whole_to_the_last() {
    let walk = walk();
    assert_eq!(walk.target(Some(3), Step::Previous), Ok(Some(1)));
    assert_eq!(walk.target(Some(0), Step::Previous), Ok(None));
    assert_eq!(walk.target(None, Step::Previous), Ok(Some(3)));
}

#[test]
fn a_commit_is_gone_to_by_its_index_unless_it_is_stepped_over() {
    let walk = walk();
    assert_eq!(walk.target(Some(0), Step::Commit(3)), Ok(Some(3)));
    assert_eq!(walk.target(Some(0), Step::Whole), Ok(None));
    // A merge and a commit that changes nothing show when asked for by their row.
    assert_eq!(walk.target(None, Step::Commit(2)), Ok(Some(2)));
    assert_eq!(walk.target(None, Step::Commit(4)), Ok(Some(4)));
    assert!(walk.target(None, Step::Commit(9)).is_err());
    assert_eq!(walk.index_of(&"4".repeat(40)), Some(3));
}

#[test]
fn a_pull_request_of_merges_alone_has_nothing_to_step_to() {
    let walk = Walk::of(
        7,
        Listed {
            commits: vec![listing(1, true, false)],
            more: false,
        },
    );
    assert_eq!(
        walk.target(None, Step::Next),
        Err("#7 has no commit to review alone".into())
    );
    assert!(walk.target(None, Step::Previous).is_err());
}

#[test]
fn the_review_used_last_goes_to_the_end() {
    let mut walk = walk();
    let (one, two) = (review_id(1), review_id(2));
    walk.used(0, one);
    walk.used(1, two);
    walk.used(0, one);
    assert_eq!(walk.open, [(1, two), (0, one)]);
}

/// A review id for these tests, the `n`th an editor opened.
fn review_id(n: u64) -> oso_core::editor::ReviewId {
    let (mut editor, project) = crate::commands::review::tests::in_project(true);
    let mut id = None;
    for _ in 0..n {
        id = Some(editor.add_review(project, "r".into()).0);
    }
    id.unwrap()
}

#[test]
fn marks_are_counted_from_a_model_that_changed_and_the_list_is_dirty_then() {
    let mut walk = walk();
    walk.dirty = false;
    let review = review_id(1);
    let mut model = Review::new();
    let hunks = vec![Hunk {
        old_start: 1,
        new_start: 1,
        removed: Vec::new(),
        added: vec![b"x\n".to_vec().into_boxed_slice()],
    }];
    let shape = std::sync::Arc::new(Shape::new(Base::Unknown, hunks).unwrap());
    let carried = model
        .carry(b"a.rs", shape)
        .run(|_, _| None, |_, _| Vec::new());
    model.apply(carried);
    let anchor = Anchor::Line {
        side: Side::New,
        number: 1,
    };
    model
        .add_comment(Author::Person, b"a.rs", anchor, "Why?".into())
        .unwrap();
    walk.count(1, review, &model);
    assert!(walk.dirty);
    let entry = &walk.entries[1];
    assert_eq!((entry.viewed, entry.hunks, entry.unsent), (0, 1, 1));
    // The same revision is not counted again.
    walk.dirty = false;
    walk.count(1, review, &model);
    assert!(!walk.dirty);
    assert_eq!(walk.commits().entries[1].unsent, 1);
}
