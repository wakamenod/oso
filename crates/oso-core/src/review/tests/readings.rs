//! The tests of decisions and readings: decisions kept by their hunks' text, snapshots applied
//! only to the reading they were taken from, and files that leave the review.
//!
//! Must not depend on: `editor`, `buffer` or anything outside the review, or I/O.

use super::*;

#[test]
fn a_decision_stays_with_its_hunks_text_and_drops_when_it_changes() {
    let base = blob("1");
    let mut review = review_of(Shape::new(base.clone(), vec![hunk(5, &["a"], 5, &["A"])]).unwrap());
    assert!(review.decide(FILE, 0, Some(Decision::Accepted)));
    assert!(review.set_viewed(FILE, 0, true));
    assert!(!review.decide(FILE, 1, Some(Decision::Rejected)));
    // The same text, moved by a hunk above: the same decision.
    read(
        &mut review,
        Shape::new(
            base.clone(),
            vec![hunk(1, &[], 1, &["top"]), hunk(5, &["a"], 6, &["A"])],
        )
        .unwrap(),
    );
    assert_eq!(review.decision(FILE, 1), Some(Decision::Accepted));
    assert!(review.viewed(FILE, 1));
    assert_eq!(review.decision(FILE, 0), None);
    // Rewritten: dropped, and counted until the person looks.
    let applied = read(
        &mut review,
        Shape::new(base, vec![hunk(5, &["a"], 5, &["A2"])]).unwrap(),
    );
    assert_eq!(applied.dropped, 1);
    assert_eq!(review.decision(FILE, 0), None);
    assert!(!review.viewed(FILE, 0));
    assert_eq!(review.dropped(FILE), 1);
    review.clear_dropped(FILE);
    assert_eq!(review.dropped(FILE), 0);
}

#[test]
fn clearing_a_decision_keeps_the_viewed_mark() {
    let mut review = review_of(diff(&["a"], &["b"], blob("1")));
    review.decide(FILE, 0, Some(Decision::Rejected));
    review.set_viewed(FILE, 0, true);
    review.decide(FILE, 0, None);
    assert_eq!(review.decision(FILE, 0), None);
    assert!(review.viewed(FILE, 0));
}

#[test]
fn a_snapshot_older_than_a_reading_is_not_applied() {
    let base = blob("1");
    let mut review = review_of(Shape::new(base.clone(), vec![hunk(1, &[], 1, &["a"])]).unwrap());
    let old = review.carry(FILE, Arc::new(Shape::new(base.clone(), vec![]).unwrap()));
    read(
        &mut review,
        Shape::new(base, vec![hunk(1, &[], 1, &["b"])]).unwrap(),
    );
    let applied = review.apply(old.run(|_, _| None, |a: &[&[u8]], b: &[&[u8]]| lcs(a, b)));
    assert!(applied.again);
    assert_eq!(review.file(FILE).unwrap().hunks().len(), 1);
}

#[test]
fn a_comment_added_after_the_snapshot_asks_for_another_carry() {
    let base = blob("1");
    let mut review = review_of(Shape::new(base.clone(), vec![hunk(1, &[], 1, &["a"])]).unwrap());
    let carry = review.carry(
        FILE,
        Arc::new(Shape::new(base, vec![hunk(1, &[], 1, &["z", "a"])]).unwrap()),
    );
    let id = review
        .add_comment(Author::Person, FILE, new_line(1), String::new())
        .unwrap();
    let applied = review.apply(carry.run(|_, _| None, |a: &[&[u8]], b: &[&[u8]]| lcs(a, b)));
    assert!(applied.again);
    // Its anchor is in the reading before: outdated until it is carried.
    assert_eq!(anchor_of(&review, id), (new_line(1), true));
    let applied = read(
        &mut review,
        Shape::new(blob("1"), vec![hunk(1, &[], 1, &["z", "a"])]).unwrap(),
    );
    assert!(!applied.again);
    assert_eq!(anchor_of(&review, id), (new_line(2), false));
}

#[test]
fn a_file_that_leaves_the_review_keeps_its_comments_outdated() {
    let base = blob("1");
    let mut review = review_of(Shape::new(base.clone(), vec![hunk(3, &[], 3, &["x"])]).unwrap());
    let id = review
        .add_comment(Author::Person, FILE, new_line(3), String::new())
        .unwrap();
    review.decide(FILE, 0, Some(Decision::Accepted));
    review.remove_file(FILE);
    assert!(review.file(FILE).is_none());
    assert_eq!(review.files().count(), 0);
    assert!(review.comment(id).unwrap().is_outdated());
    read(
        &mut review,
        Shape::new(base, vec![hunk(1, &[], 1, &["y"]), hunk(3, &[], 4, &["x"])]).unwrap(),
    );
    assert_eq!(anchor_of(&review, id), (new_line(4), false));
    assert_eq!(review.decision(FILE, 1), None);
}

#[test]
fn the_old_side_of_a_moved_base_is_asked_for_once() {
    let mut review = review_of(diff(&["a", "b"], &["a", "B"], blob("1")));
    for _ in 0..3 {
        review
            .add_comment(
                Author::Person,
                FILE,
                Anchor::Line {
                    side: Side::Old,
                    number: 2,
                },
                String::new(),
            )
            .unwrap();
    }
    let mut asked = 0;
    let carried = review
        .carry(
            FILE,
            Arc::new(diff(&["new", "a", "b"], &["new", "a", "B"], blob("2"))),
        )
        .run(
            |from, to| {
                asked += 1;
                assert_eq!((from, to), (&b"1"[..], &b"2"[..]));
                Some(base_hunks(&["a", "b"], &["new", "a", "b"]))
            },
            |a: &[&[u8]], b: &[&[u8]]| lcs(a, b),
        );
    assert_eq!(asked, 1);
    review.apply(carried);
    for comment in review.comments() {
        assert_eq!(
            (comment.anchor(), comment.is_outdated()),
            (
                Anchor::Line {
                    side: Side::Old,
                    number: 3
                },
                false
            )
        );
    }
}

#[test]
fn a_decision_made_after_the_snapshot_is_carried_too() {
    let base = blob("1");
    let mut review = review_of(Shape::new(base.clone(), vec![hunk(3, &["a"], 3, &["A"])]).unwrap());
    let next = Shape::new(
        base,
        vec![hunk(1, &[], 1, &["top"]), hunk(3, &["a"], 4, &["A"])],
    );
    let carry = review.carry(FILE, Arc::new(next.unwrap()));
    review.decide(FILE, 0, Some(Decision::Rejected));
    let applied = review.apply(carry.run(|_, _| None, |a: &[&[u8]], b: &[&[u8]]| lcs(a, b)));
    assert!(applied.again);
    assert_eq!(review.decision(FILE, 1), Some(Decision::Rejected));
}

#[test]
fn hunks_with_one_text_keep_their_own_decisions() {
    let base = blob("1");
    let twins = |shift: u32| {
        let mut hunks = vec![hunk(3, &["}"], 3, &[""]), hunk(9, &["}"], 9, &[""])];
        if shift > 0 {
            hunks.insert(0, hunk(1, &[], 1, &["top"]));
            hunks[1].new_start += 1;
            hunks[2].new_start += 1;
        }
        Shape::new(base.clone(), hunks).unwrap()
    };
    let mut review = review_of(twins(0));
    review.decide(FILE, 1, Some(Decision::Accepted));
    assert_eq!(review.decision(FILE, 0), None);
    assert_eq!(review.decision(FILE, 1), Some(Decision::Accepted));
    // Moved by a hunk above, the second twin keeps it and the first does not.
    read(&mut review, twins(1));
    assert_eq!(review.decision(FILE, 1), None);
    assert_eq!(review.decision(FILE, 2), Some(Decision::Accepted));
    // One twin gone: the first left keeps the first twin's mark, and the second's drops.
    let applied = read(
        &mut review,
        Shape::new(base.clone(), vec![hunk(3, &["}"], 3, &[""])]).unwrap(),
    );
    assert_eq!(applied.dropped, 1);
    assert_eq!(review.decision(FILE, 0), None);
}

#[test]
fn a_file_keeps_at_most_its_limit_of_earlier_readings() {
    let base = blob("1");
    let line = |i: usize| format!("v{i}");
    let mut review =
        review_of(Shape::new(base.clone(), vec![hunk(1, &[], 1, &[&line(0)])]).unwrap());
    let mut ids = Vec::new();
    for i in 0..20 {
        ids.push(
            review
                .add_comment(Author::Person, FILE, new_line(1), String::new())
                .unwrap(),
        );
        read(
            &mut review,
            Shape::new(base.clone(), vec![hunk(1, &[], 1, &[&line(i + 1)])]).unwrap(),
        );
    }
    // Every line comes back, but the comments on the oldest readings were let go.
    let all: Vec<String> = (0..=20).map(line).collect();
    read(
        &mut review,
        Shape::new(base, vec![hunk(1, &[], 1, &strs(&all))]).unwrap(),
    );
    let back: Vec<bool> = ids
        .iter()
        .map(|&id| !review.comment(id).unwrap().is_outdated())
        .collect();
    let let_go = 20 - MAX_KEPT_READINGS;
    assert!(back[..let_go].iter().all(|&back| !back), "{back:?}");
    assert!(back[let_go..].iter().all(|&back| back), "{back:?}");
}
