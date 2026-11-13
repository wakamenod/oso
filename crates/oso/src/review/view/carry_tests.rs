//! The view's comments: a file waiting for its carry keeps its section, carries for readings
//! no longer current are dropped, a carry goes in the next text, a snapshot or folding lays out
//! only the files it touches, a file that leaves with comments stays last, and the text's limit.
//!
//! Must not depend on: `app`, `dispatch` or winit; these drive the view's state.

use std::sync::Arc;

use oso_core::review::{Anchor, Author, Review, Side};

use super::cards::tests::{read, shape_of, snapshot};
use super::section::tests::{entry, hunk, text_reading};
use super::*;
use crate::review::list::Status;

/// What the view is told of the model's comments on `path`.
fn cards_of(review: &Review, path: &[u8]) -> Tell {
    let cards = snapshot(review, path);
    Tell::Cards {
        path: path.into(),
        comments: cards.comments,
        reading: cards.reading,
    }
}

/// A model with a comment on line 1 of `a`, the view showing that reading with its card, and
/// the reading.
fn commented() -> (State, Review, Arc<Reading>) {
    let mut review = Review::new();
    let reading = Arc::new(read(&mut review, b"a", vec![hunk(1, 1, &[], &[b"one\n"])]));
    let on_one = Anchor::Line {
        side: Side::New,
        number: 1,
    };
    review
        .add_comment(Author::Person, b"a", on_one, "a card".into())
        .unwrap();
    let mut state = State::new("t".into());
    state.take(Tell::File {
        reading: Arc::clone(&reading),
        carry: Carrying::Nothing,
    });
    state.take(cards_of(&review, b"a"));
    state.build();
    assert!(
        state
            .text
            .to_string()
            .contains("+one\n    \u{258d}You  #0\n")
    );
    (state, review, reading)
}

/// A newer reading of `a`, with the line commented on moved down one.
fn newer() -> Arc<Reading> {
    Arc::new(text_reading(
        b"a",
        vec![hunk(1, 1, &[], &[b"zero\n", b"one\n"])],
    ))
}

/// The carry of `a`'s comments to `reading`, matching the line by its text.
fn carried_to(review: &Review, reading: &Reading) -> Carried {
    review
        .carry(b"a", shape_of(reading))
        .run(|_, _| None, |_, new| vec![(0, new.len() - 1)])
}

#[test]
fn a_file_carried_elsewhere_keeps_its_section_until_its_carry_comes() {
    let (mut state, review, _) = commented();
    let before = state.text.to_string();
    let reading = newer();
    state.take(Tell::File {
        reading: Arc::clone(&reading),
        carry: Carrying::Elsewhere,
    });
    assert!(!state.dirty, "nothing to show until the carry comes");
    let waiting = state.build();
    assert!(waiting.text.is_none() && waiting.carried.is_empty());
    assert_eq!(state.text.to_string(), before);
    state.take(Tell::Carried {
        reading: Arc::clone(&reading),
        carried: carried_to(&review, &reading),
        cut_short: true,
    });
    let text = state.build();
    assert_eq!(text.carried.len(), 1);
    assert!(Arc::ptr_eq(&text.carried[0].0, &reading));
    let shown = state.text.to_string();
    // Placed by the carry, which the model has not applied yet.
    assert!(
        shown.contains(
            "comments not carried in time: some show as outdated\n@@ -0,0 +1,2 @@\n+zero\n+one\n    \u{258d}You  #0\n"
        ),
        "{shown}"
    );
}

#[test]
fn a_carry_for_a_reading_no_longer_current_is_dropped() {
    let (mut state, review, _) = commented();
    let (first, second) = (newer(), newer());
    for reading in [&first, &second] {
        state.take(Tell::File {
            reading: Arc::clone(reading),
            carry: Carrying::Elsewhere,
        });
    }
    state.take(Tell::Carried {
        reading: Arc::clone(&first),
        carried: carried_to(&review, &first),
        cut_short: false,
    });
    assert!(state.build().carried.is_empty());
    // The UI's fallback: carried here, for the reading that waits.
    state.take(Tell::Carry {
        reading: Arc::clone(&second),
        carry: review.carry(b"a", shape_of(&second)),
    });
    let text = state.build();
    assert_eq!(text.carried.len(), 1);
    assert!(Arc::ptr_eq(&text.carried[0].0, &second));
    assert!(state.text.to_string().contains("+zero\n"));
    // Carried again on the thread for the reading shown, as applying asked: taken too.
    state.take(Tell::Carried {
        reading: Arc::clone(&second),
        carried: carried_to(&review, &second),
        cut_short: false,
    });
    assert_eq!(state.build().carried.len(), 1);
}

#[test]
fn a_snapshot_or_folding_lays_out_only_the_files_it_touches() {
    let (mut state, mut review, _) = commented();
    let other = Arc::new(read(&mut review, b"b", vec![hunk(1, 1, &[], &[b"x\n"])]));
    review
        .add_comment(
            Author::Agent,
            b"b",
            Anchor::Hunk { index: 0 },
            "first\nsecond".into(),
        )
        .unwrap();
    state.take(Tell::File {
        reading: other,
        carry: Carrying::Nothing,
    });
    state.take(cards_of(&review, b"b"));
    state.build();
    let section = |state: &State, path: &[u8]| {
        Arc::clone(&state.files[path].rendered.as_ref().unwrap().rendered)
    };
    let (a, b) = (section(&state, b"a"), section(&state, b"b"));
    state.take(Tell::Folded(true));
    state.build();
    assert!(
        Arc::ptr_eq(&a, &section(&state, b"a")),
        "no agent card in a"
    );
    assert!(!Arc::ptr_eq(&b, &section(&state, b"b")));
    assert!(
        state
            .text
            .to_string()
            .contains("  first\n    \u{258d}  \u{2026}\n")
    );
    let b = section(&state, b"b");
    state.take(cards_of(&review, b"a"));
    state.build();
    assert!(Arc::ptr_eq(&b, &section(&state, b"b")));
}

#[test]
fn a_file_that_leaves_with_comments_stays_last_until_it_has_none() {
    let (mut state, mut review, _) = commented();
    let entries: Arc<[Entry]> = vec![entry(b"b", Status::Modified)].into();
    state.take(Tell::Listed {
        entries,
        more: 0,
        cut: false,
    });
    review.remove_file(b"a");
    state.take(cards_of(&review, b"a"));
    state.build();
    let text = state.text.to_string();
    assert!(
        text.ends_with(
            "modified  b\nreading\u{2026}\nno longer changed  a  \u{b7} 1 comment\n    \u{258d}You  #0 \u{b7} outdated, was L1 (new)\n    \u{258d}  a card\n"
        ),
        "{text}"
    );
    let id = review.comments().next().unwrap().id();
    review.remove_comment(Author::Person, id).unwrap();
    state.take(cards_of(&review, b"a"));
    state.build();
    assert!(!state.text.to_string().contains("no longer changed"));
}

#[test]
fn past_the_texts_limit_cards_show_their_labels_alone() {
    let (mut state, review, reading) = commented();
    state.max_text = 1;
    state.take(Tell::File {
        reading,
        carry: Carrying::Nothing,
    });
    state.take(cards_of(&review, b"a"));
    state.build();
    let text = state.text.to_string();
    assert!(
        text.ends_with("not shown: the review's text is past its limit\n    \u{258d}You  #0\n"),
        "{text}"
    );
}
