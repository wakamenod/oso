//! The tests of a saved review (step 6): saved and restored, it holds what it held, carries to
//! a new reading as the review it came from would, and refuses what no review saves.
//!
//! Must not depend on: `editor`, `buffer` or anything outside the review, or I/O.

use proptest::prelude::*;

use super::*;

const OTHER: &[u8] = b"src/b.rs";

/// Everything a person or agent can tell of a review: each comment and its state, and each
/// file's marks, dropped count and progress. Not the changed marks, which a restored review's
/// first carry leaves out (Q-J).
#[derive(Debug, PartialEq, Eq)]
struct Seen {
    comments: Vec<SeenComment>,
    files: Vec<SeenFile>,
    progress: (usize, usize, usize),
}

/// A file's path, its hunks' decisions and viewed marks, and its dropped count.
type SeenFile = (Vec<u8>, Vec<(Option<Decision>, bool)>, usize);

type SeenComment = (
    CommentId,
    Author,
    Vec<u8>,
    Anchor,
    bool,
    String,
    Option<CommentId>,
    bool,
    bool,
    bool,
);

fn seen(review: &Review) -> Seen {
    let comments = review
        .comments()
        .map(|comment| {
            (
                comment.id(),
                comment.author(),
                comment.path().to_vec(),
                comment.anchor(),
                comment.is_outdated(),
                comment.text().to_owned(),
                comment.reply_to(),
                comment.is_sent(),
                comment.is_resolved(),
                comment.is_read(),
            )
        })
        .collect();
    let files = review
        .files()
        .map(|(path, shape)| {
            let marks: Vec<(Option<Decision>, bool)> = (0..shape.hunks().len())
                .map(|hunk| (review.decision(path, hunk), review.viewed(path, hunk)))
                .collect();
            (path.to_vec(), marks, review.dropped(path))
        })
        .filter(|(path, marks, dropped)| {
            // Only what a save keeps: a file with a mark, a comment on its reading or a count.
            let commented = review
                .comments_on(path)
                .any(|comment| !comment.is_outdated());
            commented || *dropped > 0 || marks.iter().any(|(d, v)| d.is_some() || *v)
        })
        .collect();
    Seen {
        comments,
        files,
        progress: (0, 0, 0),
    }
}

/// [`seen`] with the progress, for reviews whose files are all saved.
fn seen_whole(review: &Review) -> Seen {
    Seen {
        progress: review.progress(),
        ..seen(review)
    }
}

fn restored(review: &Review) -> Review {
    Review::restore(review.save()).unwrap()
}

/// Carries `path`'s comments to `shape` and applies it.
fn read_at(review: &mut Review, path: &[u8], shape: &Arc<Shape>) -> Applied {
    let carried = review
        .carry(path, Arc::clone(shape))
        .run(|_, _| None, |a: &[&[u8]], b: &[&[u8]]| lcs(a, b));
    review.apply(carried)
}

/// A review with a bit of everything: the person's and an agent's comments, a reply, sent,
/// resolved and read states, an outdated comment kept on an earlier reading, a decision and a
/// viewed mark, a dropped one, and a file that left the review with its comment.
fn everything() -> Review {
    let base = blob("1");
    let first = Shape::new(
        base.clone(),
        vec![hunk(2, &["b"], 2, &["B"]), hunk(9, &[], 9, &["gone"])],
    )
    .unwrap();
    let mut review = review_of(first);
    let kept = review
        .add_comment(Author::Person, FILE, new_line(2), "why B?".into())
        .unwrap();
    review
        .add_comment(Author::Person, FILE, new_line(9), "and this?".into())
        .unwrap();
    let agent = review
        .add_comment(
            Author::Agent,
            FILE,
            Anchor::Hunk { index: 0 },
            "B is right".into(),
        )
        .unwrap();
    review
        .reply(Author::Person, agent, "thanks".into())
        .unwrap();
    review.mark_sent(&[kept]);
    review.decide(FILE, 1, Some(Decision::Accepted));
    review.set_viewed(FILE, 0, true);
    // The line 9 comment goes outdated, and the accepted hunk drops.
    read(
        &mut review,
        Shape::new(
            base.clone(),
            vec![hunk(2, &["b"], 2, &["B"]), hunk(9, &[], 9, &["GONE"])],
        )
        .unwrap(),
    );
    let other = Arc::new(Shape::new(base, vec![hunk(1, &[], 1, &["x"])]).unwrap());
    read_at(&mut review, OTHER, &other);
    let unread = review
        .add_comment(Author::Agent, OTHER, new_line(1), "new here".into())
        .unwrap();
    review.resolve(Author::Agent, unread, true).unwrap();
    review.remove_file(OTHER);
    review
}

#[test]
fn a_restored_review_holds_what_it_held() {
    let review = everything();
    let back = restored(&review);
    assert_eq!(seen_whole(&back), seen_whole(&review));
    assert_eq!(review.dropped(FILE), 1);
    assert!(back.comments().any(|comment| !comment.is_read()));
    // And saved again, the same once more.
    assert_eq!(seen_whole(&restored(&back)), seen_whole(&review));
}

#[test]
fn no_id_is_reused_after_a_restore() {
    let review = everything();
    let mut back = restored(&review);
    let largest = review.comments().map(Comment::id).max().unwrap();
    let id = back
        .add_comment(Author::Person, FILE, new_line(2), "more".into())
        .unwrap();
    assert!(id > largest);
}

#[test]
fn a_restored_review_carries_to_a_new_reading_as_the_one_it_came_from() {
    let mut review = everything();
    let mut back = restored(&review);
    // The outdated comment's line comes back, a line goes in above, and the other file
    // comes back.
    let next = Arc::new(
        Shape::new(
            blob("1"),
            vec![
                hunk(1, &[], 1, &["top"]),
                hunk(2, &["b"], 3, &["B"]),
                hunk(9, &[], 10, &["gone"]),
            ],
        )
        .unwrap(),
    );
    let other = Arc::new(Shape::new(blob("1"), vec![hunk(1, &[], 1, &["x"])]).unwrap());
    for review in [&mut review, &mut back] {
        read_at(review, FILE, &next);
        read_at(review, OTHER, &other);
    }
    assert_eq!(seen_whole(&back), seen_whole(&review));
    let outdated = back.comments().filter(|comment| comment.is_outdated());
    assert_eq!(outdated.count(), 0, "every line came back");
}

#[test]
fn the_first_reading_after_a_restore_marks_no_hunk_changed() {
    let mut back = restored(&everything());
    let next = Arc::new(
        Shape::new(
            blob("1"),
            vec![hunk(1, &[], 1, &["new"]), hunk(2, &["b"], 3, &["B"])],
        )
        .unwrap(),
    );
    read_at(&mut back, FILE, &next);
    assert!(
        !back.changed(FILE, 0),
        "the restored review is new to this run"
    );
    assert!(back.viewed(FILE, 1));
    // The reading after that marks as any does.
    let after = Arc::new(
        Shape::new(
            blob("1"),
            vec![
                hunk(1, &[], 1, &["new"]),
                hunk(2, &["b"], 3, &["B"]),
                hunk(5, &[], 6, &["newer"]),
            ],
        )
        .unwrap(),
    );
    read_at(&mut back, FILE, &after);
    assert!(back.changed(FILE, 2));
    assert!(!back.changed(FILE, 0));
}

#[test]
fn a_snapshot_marked_after_the_restore_marks_no_hunk_changed_either() {
    let mut back = restored(&everything());
    let next = Arc::new(
        Shape::new(
            blob("1"),
            vec![hunk(1, &[], 1, &["new"]), hunk(2, &["b"], 3, &["B"])],
        )
        .unwrap(),
    );
    let carried = back
        .carry(FILE, Arc::clone(&next))
        .run(|_, _| None, |a: &[&[u8]], b: &[&[u8]]| lcs(a, b));
    // A mark changed meanwhile: the current marks are carried in the snapshot's place.
    back.decide(FILE, 0, Some(Decision::Accepted));
    assert!(back.apply(carried).marks_changed);
    assert!(!back.changed(FILE, 0));
    assert_eq!(back.decision(FILE, 1), Some(Decision::Accepted));
}

#[test]
fn a_save_keeps_only_files_with_something_on_them() {
    let mut review = review_of(diff(&["a"], &["b"], blob("1")));
    let other = Arc::new(diff(&["x"], &["y"], blob("2")));
    read_at(&mut review, OTHER, &other);
    assert!(review.save().files.is_empty());
    review.set_viewed(OTHER, 0, true);
    let saved = review.save();
    let paths: Vec<&[u8]> = saved.files.iter().map(|file| &*file.path).collect();
    assert_eq!(paths, [OTHER]);
    assert!(
        Arc::ptr_eq(&saved.files[0].shape, &other),
        "a reference, not a copy"
    );
}

#[test]
fn a_comment_on_a_reading_equal_to_the_current_one_is_put_on_it() {
    let review = everything();
    let mut saved = review.save();
    // Readings read back from disk are new values, not the review's references.
    let current = Arc::new((*saved.files[0].shape).clone());
    saved.files[0].shape = Arc::clone(&current);
    for comment in &mut saved.comments {
        if !comment.outdated {
            comment.shape = comment
                .shape
                .as_ref()
                .map(|shape| Arc::new((**shape).clone()));
        }
    }
    let back = Review::restore(saved).unwrap();
    assert_eq!(seen_whole(&back), seen_whole(&review));
}

#[test]
fn a_comment_whose_reading_was_not_saved_comes_back_outdated() {
    let review = everything();
    let mut saved = review.save();
    // As for a file that may hold secrets: its readings are not written.
    saved.files.clear();
    for comment in &mut saved.comments {
        comment.shape = None;
        comment.outdated = true;
    }
    let back = Review::restore(saved).unwrap();
    assert_eq!(back.comments().count(), review.comments().count());
    assert!(back.comments().all(Comment::is_outdated));
}

#[test]
fn a_restore_keeps_at_most_its_limit_of_earlier_readings() {
    let mut saved = Saved {
        next_id: 100,
        ..Saved::default()
    };
    for id in 0..MAX_KEPT_READINGS as u64 + 4 {
        let shape = Shape::new(blob("1"), vec![hunk(1, &[], 1, &[&format!("v{id}")])]);
        saved.comments.push(SavedComment {
            id,
            author: Author::Person,
            path: FILE.into(),
            anchor: new_line(1),
            shape: Some(Arc::new(shape.unwrap())),
            text: "t".into(),
            reply_to: None,
            sent: false,
            resolved: false,
            outdated: true,
            read: true,
        });
    }
    let back = Review::restore(saved).unwrap();
    let kept: Vec<u64> = back
        .save()
        .comments
        .iter()
        .filter(|comment| comment.shape.is_some())
        .map(|comment| comment.id)
        .collect();
    assert_eq!(kept, (4..MAX_KEPT_READINGS as u64 + 4).collect::<Vec<_>>());
}

#[test]
fn a_restore_refuses_comments_no_review_saves() {
    let first = |saved: &Saved| CommentId(saved.comments[0].id);
    let id = first(&everything().save());
    assert_eq!(
        refused(&|saved| saved.comments[0].text = "x".repeat(MAX_COMMENT_BYTES + 1).into()),
        RestoreError::TooLong { id }
    );
    assert_eq!(
        refused(&|saved| saved.comments[1].id = saved.comments[0].id),
        RestoreError::SameId { id }
    );
    assert_eq!(
        refused(&|saved| saved.next_id = 0),
        RestoreError::IdTooLarge { id }
    );
    let reply = |saved: &Saved| {
        saved
            .comments
            .iter()
            .position(|comment| comment.reply_to.is_some())
            .unwrap()
    };
    let reply_id = CommentId(everything().save().comments[reply(&everything().save())].id);
    assert_eq!(
        refused(&|saved| {
            let at = reply(saved);
            saved.comments[at].reply_to = Some(999);
        }),
        RestoreError::NoParent { id: reply_id }
    );
    assert_eq!(
        refused(&|saved| {
            let at = reply(saved);
            saved.comments[at].reply_to = Some(reply_id.get());
        }),
        RestoreError::NoParent { id: reply_id },
        "a reply to itself"
    );
    assert_eq!(
        refused(&|saved| {
            let at = reply(saved);
            saved.comments[at].path = OTHER.into();
        }),
        RestoreError::NoParent { id: reply_id },
        "a reply on another file"
    );
    assert_eq!(
        refused(&|saved| saved.comments[0].anchor = new_line(50)),
        RestoreError::NotInReading { id }
    );
    assert_eq!(
        refused(&|saved| {
            saved.comments[0].shape = Some(Arc::new(diff(&["q"], &["r"], blob("9"))));
            saved.comments[0].anchor = new_line(1);
        }),
        RestoreError::NotInReading { id },
        "not outdated, on a reading that is not the current one"
    );
}

/// [`everything`]'s save, changed by `change`, and why it is refused.
fn refused(change: &dyn Fn(&mut Saved)) -> RestoreError {
    let mut saved = everything().save();
    change(&mut saved);
    Review::restore(saved).unwrap_err()
}

#[test]
fn a_restore_refuses_files_and_counts_no_review_saves() {
    assert_eq!(
        refused(&|saved| {
            let file = saved.files[0].clone();
            saved.files.push(file);
        }),
        RestoreError::SameFile { file: 1 }
    );
    assert_eq!(
        refused(&|saved| {
            saved.files[0].marks.pop();
        }),
        RestoreError::Marks { file: 0 }
    );
    let mut many = Saved {
        next_id: MAX_COMMENTS as u64 + 2,
        ..Saved::default()
    };
    let shape = Arc::new(diff(&["a"], &["b"], blob("1")));
    many.comments = (0..=MAX_COMMENTS as u64)
        .map(|id| SavedComment {
            id,
            author: Author::Agent,
            path: FILE.into(),
            anchor: new_line(1),
            shape: Some(Arc::clone(&shape)),
            text: "".into(),
            reply_to: None,
            sent: false,
            resolved: false,
            outdated: true,
            read: false,
        })
        .collect();
    assert_eq!(
        Review::restore(many).unwrap_err(),
        RestoreError::TooManyComments
    );
}

#[test]
fn a_long_line_is_saved_whole_and_still_compared_whole() {
    let long = "x".repeat(64 * 1024);
    let other = format!("{}y", &long[..long.len() - 1]);
    let mut review = review_of(Shape::new(blob("1"), vec![hunk(1, &[], 1, &[&long])]).unwrap());
    let id = review
        .add_comment(Author::Person, FILE, new_line(1), "long".into())
        .unwrap();
    let mut back = restored(&review);
    let mut again = restored(&review);
    let changed = Arc::new(Shape::new(blob("1"), vec![hunk(1, &[], 1, &[&other])]).unwrap());
    read_at(&mut back, FILE, &changed);
    assert!(
        back.comment(id).unwrap().is_outdated(),
        "its last byte differs"
    );
    let moved = Arc::new(Shape::new(blob("1"), vec![hunk(1, &[], 1, &["a", &long])]).unwrap());
    read_at(&mut again, FILE, &moved);
    assert_eq!(anchor_of(&again, id), (new_line(2), false));
}

#[test]
fn the_persons_comments_come_back_read() {
    let review = everything();
    let mut saved = review.save();
    for comment in &mut saved.comments {
        comment.read = false;
    }
    let back = Review::restore(saved).unwrap();
    for comment in back.comments() {
        assert_eq!(comment.is_read(), comment.author() == Author::Person);
    }
}

/// Lines of a file's readings: line `i` of version `v` reads `l{i}` or, changed, `l{i}.{v}`.
fn version(changes: &[(u8, bool)], number: u8) -> Vec<String> {
    changes
        .iter()
        .enumerate()
        .map(|(i, &(at, changed))| {
            if changed && at <= number {
                format!("l{i}.{at}")
            } else {
                format!("l{i}")
            }
        })
        .collect()
}

proptest! {
    /// Whatever was done to a review, saved and restored it carries every later reading as the
    /// review it came from does.
    #[test]
    fn a_restored_review_follows_later_readings_as_the_original(
        changes in proptest::collection::vec((0u8..4, any::<bool>()), 3..10),
        steps in proptest::collection::vec((0u8..5, 0usize..12), 1..24),
        restore_at in 0usize..24,
    ) {
        let old: Vec<String> = (0..changes.len()).map(|i| format!("l{i}")).collect();
        let reading = |number: u8| {
            Arc::new(diff(&strs(&old), &strs(&version(&changes, number)), blob("1")))
        };
        let mut review = Review::new();
        read_at(&mut review, FILE, &reading(1));
        let mut back: Option<Review> = None;
        let mut number = 1;
        let mut ids: Vec<CommentId> = Vec::new();
        for (step, (action, target)) in steps.into_iter().enumerate() {
            if step == restore_at {
                // Opened again, a review reads every file first.
                let mut again = restored(&review);
                read_at(&mut again, FILE, &reading(number));
                back = Some(again);
            }
            let reviews: Vec<&mut Review> =
                std::iter::once(&mut review).chain(back.as_mut()).collect();
            let hunks = review_hunks(&reviews);
            for review in reviews {
                match action {
                    0 if hunks > 0 => {
                        let index = target % hunks;
                        let _ = review.add_comment(
                            Author::Person, FILE, Anchor::Hunk { index }, "h".into());
                    }
                    1 => {
                        let number = u32::try_from(target).unwrap() + 1;
                        let _ = review.add_comment(Author::Agent, FILE, new_line(number), "l".into());
                    }
                    2 if !ids.is_empty() => {
                        let _ = review.reply(Author::Person, ids[target % ids.len()], "r".into());
                    }
                    3 if hunks > 0 => {
                        review.set_viewed(FILE, target % hunks, true);
                        review.decide(FILE, target % hunks, Some(Decision::Accepted));
                    }
                    _ => {}
                }
            }
            if action == 4 {
                number = (number % 3) + 1;
                let next = reading(number);
                read_at(&mut review, FILE, &next);
                if let Some(back) = back.as_mut() {
                    read_at(back, FILE, &next);
                }
            }
            ids = review.comments().map(Comment::id).collect();
            if let Some(back) = &back {
                prop_assert_eq!(seen(back), seen(&review));
            }
        }
    }
}

/// The hunks the reviews' current reading of [`FILE`] has, the same in each.
fn review_hunks(reviews: &[&mut Review]) -> usize {
    reviews[0].file(FILE).map_or(0, |shape| shape.hunks().len())
}

#[test]
fn every_change_of_a_review_counts_and_a_save_does_not() {
    let mut review = review_of(diff(&["a"], &["b"], blob("1")));
    let mut last = review.revision();
    let mut changed = |review: &Review, what: &str| {
        assert_ne!(review.revision(), last, "{what}");
        last = review.revision();
    };
    let id = review
        .add_comment(Author::Agent, FILE, new_line(1), "c".into())
        .unwrap();
    changed(&review, "a comment");
    review.edit_comment(Author::Agent, id, "d".into()).unwrap();
    changed(&review, "an edit");
    review.resolve(Author::Agent, id, true).unwrap();
    changed(&review, "a resolve");
    review.mark_read(&[id]);
    changed(&review, "read");
    review.mark_sent(&[id]);
    changed(&review, "sent");
    review.set_viewed(FILE, 0, true);
    changed(&review, "viewed");
    review.decide(FILE, 0, Some(Decision::Accepted));
    changed(&review, "a decision");
    read(&mut review, diff(&["a"], &["c"], blob("1")));
    changed(&review, "a reading");
    review.clear_dropped(FILE);
    changed(&review, "dropped cleared");
    review.remove_comment(Author::Agent, id).unwrap();
    changed(&review, "a removal");
    review.remove_file(FILE);
    changed(&review, "a file gone");
    let _ = review.save();
    assert_eq!(review.revision(), last);
    assert!(review.edit_comment(Author::Person, id, "x".into()).is_err());
    assert_eq!(review.revision(), last, "a refused change counts nothing");
}
