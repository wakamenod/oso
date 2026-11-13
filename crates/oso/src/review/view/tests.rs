//! The view's whole text from what it is told: files listed then read, files that go, and the
//! thread's batches and end.
//!
//! Must not depend on: `app`, `dispatch` or winit; these drive the view's state and thread.

use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use oso_core::review::{Review, RowKind};

use super::section::tests::{entry, hunk, text_reading};
use super::*;
use crate::review::list::Status;

#[test]
fn listed_files_show_until_read_then_their_sections() {
    let mut state = State::new("Review \u{b7} uncommitted \u{b7} oso".into());
    let entries: Arc<[Entry]> =
        vec![entry(b"a", Status::Modified), entry(b"b", Status::Added)].into();
    state.take(Tell::Listed { entries, more: 3 });
    let first = state.build();
    assert_eq!(first.reading, Some((0, 2)));
    let review = Review::new();
    let reading = Arc::new(text_reading(b"b", vec![hunk(1, 1, &[], &[b"x\n"])]));
    let shape = match &reading.content {
        crate::review::Content::Text { shape, .. } => Arc::clone(shape),
        _ => unreachable!(),
    };
    state.take(Tell::File {
        reading,
        carry: Some(review.carry(b"b", shape)),
    });
    state.take(Tell::Read);
    let second = state.build();
    assert_eq!(second.reading, None);
    assert_eq!(second.carried.len(), 1);
    assert!(
        second
            .text
            .as_ref()
            .is_some_and(oso_core::load::Reloaded::is_changed)
    );
    let rows = &second.rows;
    let kinds: Vec<RowKind> = (0..rows.lines())
        .map(|line| rows.at(line).unwrap().kind)
        .collect();
    assert_eq!(
        kinds.iter().filter(|kind| **kind == RowKind::File).count(),
        2
    );
    assert_eq!(second.highlights.len(), state.text.len());
    assert_eq!(rows.lines(), state.text.max_point().row);
    let text = state.text.to_string();
    assert!(
        text.starts_with("Review \u{b7} uncommitted \u{b7} oso\n3 more files are not listed\n")
    );
    assert!(
        text.contains(
            "modified  a\nreading\u{2026}\nmodified  b  +1 \u{2212}0\n@@ -0,0 +1 @@\n+x\n"
        )
    );
    // Nothing changed: the same text, compared as the same.
    let third = state.build();
    assert!(
        third.text.is_none(),
        "the same valid text is not sent again"
    );
}

#[test]
fn a_carry_again_runs_only_for_the_files_current_reading() {
    let mut state = State::new("t".into());
    let review = Review::new();
    let first = Arc::new(text_reading(b"a", vec![hunk(1, 1, &[b"x\n"], &[])]));
    let second = Arc::new(text_reading(b"a", vec![hunk(1, 1, &[], &[b"y\n"])]));
    let shape = |reading: &Reading| match &reading.content {
        crate::review::Content::Text { shape, .. } => Arc::clone(shape),
        _ => unreachable!(),
    };
    state.take(Tell::File {
        reading: Arc::clone(&second),
        carry: None,
    });
    // A carry asked again for a reading a newer one replaced is dropped.
    state.take(Tell::Carry {
        reading: Arc::clone(&first),
        carry: review.carry(b"a", shape(&first)),
    });
    assert!(state.build().carried.is_empty());
    state.take(Tell::Carry {
        reading: Arc::clone(&second),
        carry: review.carry(b"a", shape(&second)),
    });
    let carried = state.build().carried;
    assert_eq!(carried.len(), 1);
    assert!(Arc::ptr_eq(&carried[0].0, &second));
}

#[test]
fn a_file_no_longer_listed_goes() {
    let mut state = State::new("t".into());
    state.take(Tell::File {
        reading: Arc::new(text_reading(b"a", vec![hunk(1, 1, &[b"x\n"], &[])])),
        carry: None,
    });
    let entries: Arc<[Entry]> = vec![entry(b"b", Status::Modified)].into();
    state.take(Tell::Listed { entries, more: 0 });
    state.build();
    let text = state.text.to_string();
    assert!(!text.contains("  a"), "{text}");
    assert!(text.contains("modified  b\n"));
}

#[test]
fn the_thread_sends_a_text_for_each_batch_and_ends_when_dropped() {
    let (sent, texts) = mpsc::channel();
    let view = View::start("t".into(), move |text| sent.send(text).is_ok()).unwrap();
    assert!(view.tell(Tell::Said(vec!["the project is no longer trusted".into()])));
    let text = texts.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(text.rows.lines(), 2);
    drop(view);
    assert!(texts.recv_timeout(Duration::from_secs(10)).is_err());
}
