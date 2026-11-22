//! A pull request's walk through its commits (ADR 0167): where each step goes, the merges and
//! commits that change nothing stepped over, the edges, the reviews used last, and the marks
//! counted only from a model that changed.
//!
//! Must not depend on: the app's threads or windows; a walk is plain data here.

use oso_core::editor::Step;
use oso_core::review::{Anchor, Author, Base, Hunk, Review, Shape, Side};

use super::{Spot, Walk};
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

/// A listing of `commits`.
fn listed(commits: Vec<Listing>) -> Listed {
    Listed {
        commits,
        more: false,
        base: None,
        tip: None,
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
    Walk::of(12, listed(commits))
}

#[test]
fn next_goes_from_the_whole_to_the_first_and_steps_over_merges() {
    let walk = walk();
    assert_eq!(walk.spot_to(Spot::Whole, Step::Next), Ok(Spot::Commit(0)));
    assert_eq!(
        walk.spot_to(Spot::Commit(0), Step::Next),
        Ok(Spot::Commit(1))
    );
    assert_eq!(
        walk.spot_to(Spot::Commit(1), Step::Next),
        Ok(Spot::Commit(3)),
        "the merge stepped over"
    );
    // The last that changes something is the end.
    assert_eq!(
        walk.spot_to(Spot::Commit(3), Step::Next),
        Err("4444444 is the last commit of #12".into())
    );
}

#[test]
fn previous_goes_from_the_first_to_the_whole_and_from_the_whole_to_the_last() {
    let walk = walk();
    assert_eq!(
        walk.spot_to(Spot::Commit(3), Step::Previous),
        Ok(Spot::Commit(1))
    );
    assert_eq!(
        walk.spot_to(Spot::Commit(0), Step::Previous),
        Ok(Spot::Whole)
    );
    assert_eq!(
        walk.spot_to(Spot::Whole, Step::Previous),
        Ok(Spot::Commit(3))
    );
}

#[test]
fn a_commit_is_gone_to_by_its_index_unless_it_is_stepped_over() {
    let walk = walk();
    assert_eq!(
        walk.spot_to(Spot::Commit(0), Step::Commit(3)),
        Ok(Spot::Commit(3))
    );
    assert_eq!(walk.spot_to(Spot::Commit(0), Step::Whole), Ok(Spot::Whole));
    // A merge and a commit that changes nothing show when asked for by their row.
    assert_eq!(
        walk.spot_to(Spot::Whole, Step::Commit(2)),
        Ok(Spot::Commit(2))
    );
    assert_eq!(
        walk.spot_to(Spot::Whole, Step::Commit(4)),
        Ok(Spot::Commit(4))
    );
    assert!(walk.spot_to(Spot::Whole, Step::Commit(9)).is_err());
    assert_eq!(walk.index_of(&"4".repeat(40)), Some(3));
}

#[test]
fn a_pull_request_of_merges_alone_has_nothing_to_step_to() {
    let walk = Walk::of(7, listed(vec![listing(1, true, false)]));
    assert_eq!(
        walk.spot_to(Spot::Whole, Step::Next),
        Err("#7 has no commit to review alone".into())
    );
    assert!(walk.spot_to(Spot::Whole, Step::Previous).is_err());
}

#[test]
fn a_range_goes_from_its_lower_to_its_higher_and_steps_on_from_its_ends() {
    let walk = walk();
    assert_eq!(
        walk.spot_to(Spot::Whole, Step::Range(3, 1)),
        Ok(Spot::Range(1, 3))
    );
    assert_eq!(
        walk.spot_to(Spot::Whole, Step::Range(1, 1)),
        Ok(Spot::Commit(1))
    );
    assert!(walk.spot_to(Spot::Whole, Step::Range(1, 9)).is_err());
    assert_eq!(
        walk.spot_to(Spot::Range(0, 1), Step::Next),
        Ok(Spot::Commit(3))
    );
    assert_eq!(
        walk.spot_to(Spot::Range(1, 3), Step::Previous),
        Ok(Spot::Commit(0))
    );
}

#[test]
fn since_the_last_review_steps_to_the_first_new_commit_and_back_to_the_whole() {
    let mut walk = walk();
    assert!(
        walk.spot_to(Spot::Whole, Step::Since).is_err(),
        "nothing recorded"
    );
    let mut entries = walk.entries.to_vec();
    entries[2].new = true;
    entries[3].new = true;
    walk.entries = std::sync::Arc::from(entries);
    walk.since = Some(oso_core::editor::Since {
        new: 2,
        rebased: false,
        gone: false,
        shown: false,
        review: None,
    });
    assert_eq!(walk.spot_to(Spot::Whole, Step::Since), Ok(Spot::Since));
    // The merge is new but stepped over: the first new commit to review is the fourth.
    assert_eq!(walk.spot_to(Spot::Since, Step::Next), Ok(Spot::Commit(3)));
    assert_eq!(walk.spot_to(Spot::Since, Step::Previous), Ok(Spot::Whole));
    walk.since = walk.since.map(|since| oso_core::editor::Since {
        gone: true,
        ..since
    });
    assert!(walk.spot_to(Spot::Whole, Step::Since).is_err(), "gone");
    // Shown last: the walk knows its spot.
    walk.show(Spot::Since);
    assert_eq!(walk.spot(), Spot::Since);
    assert!(walk.commits().since.is_some_and(|since| since.shown));
    walk.none_new();
    assert!(walk.commits().since.is_none());
    assert!(walk.entries.iter().all(|entry| !entry.new));
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
