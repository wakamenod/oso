//! The tests of comments: carried across readings, kept within their limits, and changed only
//! as their authors may.
//!
//! Must not depend on: `editor`, `buffer` or anything outside the review, or I/O.

use proptest::prelude::*;

use super::*;

#[test]
fn a_line_comment_follows_its_line_and_goes_outdated_when_it_changes() {
    let base = blob("b1");
    let mut review =
        review_of(Shape::new(base.clone(), vec![hunk(7, &[], 7, &["one", "two"])]).unwrap());
    let id = review
        .add_comment(Author::Person, FILE, new_line(8), "why two?".into())
        .unwrap();
    // A line put in above moves it down.
    let applied = read(
        &mut review,
        Shape::new(
            base.clone(),
            vec![hunk(1, &[], 1, &["use"]), hunk(7, &[], 8, &["one", "two"])],
        )
        .unwrap(),
    );
    assert_eq!(applied.placed, 1);
    assert_eq!(anchor_of(&review, id), (new_line(9), false));
    // Its line rewritten: outdated, kept where it was.
    let applied = read(
        &mut review,
        Shape::new(base.clone(), vec![hunk(7, &[], 7, &["one", "TWO"])]).unwrap(),
    );
    assert_eq!(applied.outdated, 1);
    assert_eq!(anchor_of(&review, id), (new_line(9), true));
    // And back.
    let applied = read(
        &mut review,
        Shape::new(base, vec![hunk(7, &[], 7, &["one", "two"])]).unwrap(),
    );
    assert_eq!((applied.placed, applied.returned), (1, 1));
    assert_eq!(anchor_of(&review, id), (new_line(8), false));
}

#[test]
fn a_comment_needs_a_line_the_diff_shows() {
    let mut review = review_of(diff(&["a", "b"], &["a", "B"], blob("1")));
    assert_eq!(
        review.add_comment(Author::Agent, FILE, new_line(1), String::new()),
        Err(CommentError::NotInDiff)
    );
    assert_eq!(
        review.add_comment(
            Author::Agent,
            FILE,
            Anchor::Hunk { index: 1 },
            String::new()
        ),
        Err(CommentError::NotInDiff)
    );
    assert_eq!(
        review.add_comment(Author::Agent, b"other", new_line(2), String::new()),
        Err(CommentError::NoSuchFile)
    );
    assert_eq!(
        review.add_comment(
            Author::Agent,
            FILE,
            new_line(2),
            "x".repeat(MAX_COMMENT_BYTES + 1)
        ),
        Err(CommentError::TooLong)
    );
    assert!(
        review
            .add_comment(
                Author::Agent,
                FILE,
                new_line(2),
                "x".repeat(MAX_COMMENT_BYTES)
            )
            .is_ok()
    );
}

#[test]
fn a_review_holds_at_most_its_limit_of_comments() {
    let mut review = review_of(diff(&["a"], &["b"], blob("1")));
    for _ in 0..MAX_COMMENTS {
        review
            .add_comment(Author::Agent, FILE, new_line(1), String::new())
            .unwrap();
    }
    assert_eq!(
        review.add_comment(Author::Person, FILE, new_line(1), String::new()),
        Err(CommentError::TooMany)
    );
}

#[test]
fn a_long_line_is_compared_whole() {
    let long = "x".repeat(MAX_KEPT_LINE + 10);
    let other = format!("{}y", &long[..long.len() - 1]);
    let base = blob("1");
    let mut review = review_of(Shape::new(base.clone(), vec![hunk(1, &[], 1, &[&long])]).unwrap());
    let id = review
        .add_comment(Author::Person, FILE, new_line(1), "long".into())
        .unwrap();
    read(
        &mut review,
        Shape::new(base.clone(), vec![hunk(1, &[], 1, &[&other])]).unwrap(),
    );
    assert!(review.comment(id).unwrap().is_outdated());
    read(
        &mut review,
        Shape::new(base, vec![hunk(1, &[], 1, &["a", &long])]).unwrap(),
    );
    assert_eq!(anchor_of(&review, id), (new_line(2), false));
}

#[test]
fn a_hunk_comment_goes_where_most_of_its_lines_went() {
    let base = blob("1");
    let mut review = review_of(
        Shape::new(
            base.clone(),
            vec![hunk(3, &["a", "b"], 3, &["A", "B", "C", "D"])],
        )
        .unwrap(),
    );
    let id = review
        .add_comment(Author::Person, FILE, Anchor::Hunk { index: 0 }, "hm".into())
        .unwrap();
    // Five of six stayed: "a" and "A" in the second hunk, "B" to "D" in the third.
    read(
        &mut review,
        Shape::new(
            base.clone(),
            vec![
                hunk(1, &[], 1, &["top"]),
                hunk(3, &["a"], 4, &["A"]),
                hunk(5, &[], 6, &["B", "C", "D"]),
            ],
        )
        .unwrap(),
    );
    assert_eq!(anchor_of(&review, id), (Anchor::Hunk { index: 2 }, false));
}

#[test]
fn a_hunk_comment_tie_goes_to_the_first_hunk_voted() {
    let base = blob("1");
    let mut review = review_of(
        Shape::new(
            base.clone(),
            vec![hunk(3, &["o"], 3, &["A", "B", "C", "D"])],
        )
        .unwrap(),
    );
    let id = review
        .add_comment(Author::Person, FILE, Anchor::Hunk { index: 0 }, "hm".into())
        .unwrap();
    // Old line 3 is kept now, between A and B and C and D: four of five stayed, two each.
    read(
        &mut review,
        Shape::new(
            base,
            vec![hunk(3, &[], 3, &["A", "B"]), hunk(4, &[], 6, &["C", "D"])],
        )
        .unwrap(),
    );
    assert_eq!(anchor_of(&review, id), (Anchor::Hunk { index: 0 }, false));
}

#[test]
fn a_hunk_comment_whose_lines_mostly_went_is_outdated() {
    let base = blob("1");
    let mut review =
        review_of(Shape::new(base.clone(), vec![hunk(3, &[], 3, &["A", "B"])]).unwrap());
    let id = review
        .add_comment(Author::Person, FILE, Anchor::Hunk { index: 0 }, "hm".into())
        .unwrap();
    // A went above line 1, so only B stayed: half went.
    read(
        &mut review,
        Shape::new(base, vec![hunk(1, &[], 1, &["A"]), hunk(3, &[], 4, &["B"])]).unwrap(),
    );
    assert_eq!(anchor_of(&review, id), (Anchor::Hunk { index: 0 }, true));
}

proptest! {
    /// A hunk comment is outdated exactly when half or more of its lines went.
    #[test]
    fn a_hunk_comment_is_outdated_when_half_its_lines_went(
        size in 1usize..12,
        changed in proptest::collection::vec(any::<bool>(), 12),
    ) {
        let base = blob("1");
        let first: Vec<String> = (0..size).map(|i| format!("line {i}")).collect();
        let next: Vec<String> = first
            .iter()
            .zip(&changed)
            .map(|(line, &changed)| if changed { format!("{line}!") } else { line.clone() })
            .collect();
        let first_reading = Shape::new(base.clone(), vec![hunk(1, &[], 1, &strs(&first))]);
        let mut review = review_of(first_reading.unwrap());
        let id = review
            .add_comment(Author::Person, FILE, Anchor::Hunk { index: 0 }, String::new())
            .unwrap();
        read(&mut review, Shape::new(base, vec![hunk(1, &[], 1, &strs(&next))]).unwrap());
        let went = changed[..size].iter().filter(|&&changed| changed).count();
        prop_assert_eq!(review.comment(id).unwrap().is_outdated(), 2 * went >= size);
    }
}

#[test]
fn an_agent_never_changes_the_persons_comments() {
    let mut review = review_of(diff(&["a"], &["b"], blob("1")));
    let person = review
        .add_comment(Author::Person, FILE, new_line(1), "mine".into())
        .unwrap();
    let agent = review
        .add_comment(Author::Agent, FILE, new_line(1), "noted".into())
        .unwrap();
    assert_eq!(
        review.edit_comment(Author::Agent, person, "gone".into()),
        Err(CommentError::NotYours)
    );
    assert_eq!(
        review.resolve(Author::Agent, person, true),
        Err(CommentError::NotYours)
    );
    assert_eq!(
        review.remove_comment(Author::Agent, person),
        Err(CommentError::NotYours)
    );
    // The person edits only their own, but resolves any.
    assert_eq!(
        review.edit_comment(Author::Person, agent, "x".into()),
        Err(CommentError::NotYours)
    );
    assert_eq!(review.resolve(Author::Person, agent, true), Ok(()));
    // A reply of the person's keeps an agent from removing its own comment.
    let reply = review.reply(Author::Person, agent, "no".into()).unwrap();
    assert_eq!(
        review.remove_comment(Author::Agent, agent),
        Err(CommentError::RepliesNotYours)
    );
    assert_eq!(review.comment(reply).unwrap().reply_to(), Some(agent));
    // The person removes the thread whole.
    assert_eq!(review.remove_comment(Author::Person, agent), Ok(2));
    assert!(review.comment(reply).is_none());
    assert_eq!(review.comment(person).unwrap().text(), "mine");
}

#[test]
fn editing_a_sent_comment_makes_it_unsent() {
    let mut review = review_of(diff(&["a"], &["b"], blob("1")));
    let id = review
        .add_comment(Author::Person, FILE, new_line(1), "one".into())
        .unwrap();
    review.mark_sent(&[id]);
    assert!(review.comment(id).unwrap().is_sent());
    review
        .edit_comment(Author::Person, id, "two".into())
        .unwrap();
    assert!(!review.comment(id).unwrap().is_sent());
    let agent = review
        .add_comment(Author::Agent, FILE, new_line(1), "hi".into())
        .unwrap();
    assert!(!review.comment(agent).unwrap().is_read());
    review.mark_read(&[agent]);
    assert!(review.comment(agent).unwrap().is_read());
}

#[test]
fn a_reply_takes_its_parents_place_and_follows_it() {
    let base = blob("1");
    let mut review = review_of(Shape::new(base.clone(), vec![hunk(2, &[], 2, &["x"])]).unwrap());
    let parent = review
        .add_comment(Author::Agent, FILE, new_line(2), "?".into())
        .unwrap();
    let reply = review.reply(Author::Person, parent, "!".into()).unwrap();
    read(
        &mut review,
        Shape::new(
            base,
            vec![hunk(1, &[], 1, &["a", "b"]), hunk(2, &[], 4, &["x"])],
        )
        .unwrap(),
    );
    assert_eq!(anchor_of(&review, parent), (new_line(4), false));
    assert_eq!(anchor_of(&review, reply), (new_line(4), false));
}

#[test]
fn ids_are_never_reused() {
    let mut review = review_of(diff(&["a"], &["b"], blob("1")));
    let first = review
        .add_comment(Author::Person, FILE, new_line(1), String::new())
        .unwrap();
    review.remove_comment(Author::Person, first).unwrap();
    let second = review
        .add_comment(Author::Person, FILE, new_line(1), String::new())
        .unwrap();
    assert!(second.get() > first.get());
}

proptest! {
    /// Whatever an agent does, the person's comments keep their text and state.
    #[test]
    fn no_agent_command_touches_a_persons_comment(
        steps in proptest::collection::vec((any::<bool>(), 0u8..6, 0usize..8), 1..40),
    ) {
        let mut review = review_of(diff(&["a", "b"], &["A", "B"], blob("1")));
        let mut ids: Vec<CommentId> = Vec::new();
        for (by_person, action, target) in steps {
            let by = if by_person { Author::Person } else { Author::Agent };
            let before: Vec<(CommentId, String, bool)> = review
                .comments()
                .filter(|comment| comment.author() == Author::Person)
                .map(|comment| (comment.id(), comment.text().to_owned(), comment.is_resolved()))
                .collect();
            let target = ids.get(target % ids.len().max(1)).copied();
            let made = match (action, target) {
                (0, _) | (_, None) => review.add_comment(by, FILE, new_line(1), "n".into()).ok(),
                (1, Some(id)) => review.reply(by, id, "r".into()).ok(),
                (2, Some(id)) => review.edit_comment(by, id, "e".into()).ok().map(|()| id),
                (3, Some(id)) => review.resolve(by, id, true).ok().map(|()| id),
                (_, Some(id)) => review.remove_comment(by, id).ok().map(|_| id),
            };
            if let Some(id) = made
                && !ids.contains(&id)
            {
                prop_assert!(ids.iter().all(|old| old.get() < id.get()));
                ids.push(id);
            }
            if by == Author::Agent {
                for (id, text, resolved) in before {
                    let comment = review.comment(id);
                    prop_assert!(comment.is_some());
                    let comment = comment.unwrap();
                    prop_assert_eq!(comment.text(), text.as_str());
                    prop_assert_eq!(comment.is_resolved(), resolved);
                }
            }
        }
    }
}

#[test]
fn an_agent_resolves_its_own_only_while_the_person_has_not_replied() {
    let mut review = review_of(diff(&["a"], &["b"], blob("1")));
    let agent = review
        .add_comment(Author::Agent, FILE, new_line(1), "noted".into())
        .unwrap();
    assert_eq!(review.resolve(Author::Agent, agent, true), Ok(()));
    assert_eq!(review.resolve(Author::Agent, agent, false), Ok(()));
    review
        .reply(Author::Person, agent, "not yet".into())
        .unwrap();
    assert_eq!(
        review.resolve(Author::Agent, agent, true),
        Err(CommentError::RepliesNotYours)
    );
    review.resolve(Author::Person, agent, true).unwrap();
    assert_eq!(
        review.resolve(Author::Agent, agent, false),
        Err(CommentError::RepliesNotYours)
    );
    assert!(review.comment(agent).unwrap().is_resolved());
}

#[test]
fn a_comment_on_a_removed_line_follows_it() {
    let base = blob("1");
    let old_line = |number| Anchor::Line {
        side: Side::Old,
        number,
    };
    let mut review = review_of(Shape::new(base.clone(), vec![hunk(4, &["gone"], 4, &[])]).unwrap());
    let id = review
        .add_comment(Author::Person, FILE, old_line(4), "why?".into())
        .unwrap();
    // Still taken out, with more taken out above it.
    read(
        &mut review,
        Shape::new(
            base.clone(),
            vec![hunk(1, &["a"], 1, &[]), hunk(4, &["gone"], 3, &[])],
        )
        .unwrap(),
    );
    assert_eq!(anchor_of(&review, id), (old_line(4), false));
    // Kept after all: outdated, and back when it is taken out again.
    read(
        &mut review,
        Shape::new(base.clone(), vec![hunk(1, &["a"], 1, &[])]).unwrap(),
    );
    assert!(review.comment(id).unwrap().is_outdated());
    read(
        &mut review,
        Shape::new(base, vec![hunk(4, &["gone"], 4, &["new"])]).unwrap(),
    );
    assert_eq!(anchor_of(&review, id), (old_line(4), false));
}

#[test]
fn a_comment_on_a_new_file_goes_outdated_once_the_file_is_committed() {
    let file = ["fn a() {", "}", "fn b() {", "}"];
    let mut review = review_of(Shape::new(Base::Empty, vec![hunk(1, &[], 1, &file)]).unwrap());
    let line = review
        .add_comment(Author::Person, FILE, new_line(4), "brace".into())
        .unwrap();
    let whole = review
        .add_comment(
            Author::Person,
            FILE,
            Anchor::Hunk { index: 0 },
            "all".into(),
        )
        .unwrap();
    read(
        &mut review,
        Shape::new(blob("1"), vec![hunk(5, &[], 5, &["fn c() {", "}"])]).unwrap(),
    );
    assert_eq!(anchor_of(&review, line), (new_line(4), true));
    assert_eq!(anchor_of(&review, whole), (Anchor::Hunk { index: 0 }, true));
}
