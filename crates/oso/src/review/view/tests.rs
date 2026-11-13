//! The view's whole text from what it is told: files listed then read, files that go, nothing
//! laid out when nothing changed, and the thread's batches, one in flight, and end.
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
    state.take(Tell::Listed {
        entries,
        more: 3,
        cut: false,
    });
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
    state.take(Tell::Listed {
        entries,
        more: 0,
        cut: false,
    });
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
    view.taken();
    drop(view);
    assert!(texts.recv_timeout(Duration::from_secs(10)).is_err());
}

#[test]
fn nothing_is_laid_out_again_when_nothing_in_the_text_changed() {
    let mut state = State::new("t".into());
    let entries: Arc<[Entry]> = vec![entry(b"a", Status::Modified)].into();
    state.take(Tell::Listed {
        entries: Arc::clone(&entries),
        more: 0,
        cut: false,
    });
    state.take(Tell::File {
        reading: Arc::new(text_reading(b"a", vec![hunk(1, 1, &[b"x\n"], &[])])),
        carry: None,
    });
    let first = state.build();
    assert!(first.text.is_some());
    // g with nothing changed: the same listing, then the end of the reading.
    state.take(Tell::Listed {
        entries,
        more: 0,
        cut: false,
    });
    assert!(!state.dirty, "the same listing changes no text");
    let listed = state.build();
    assert!(listed.text.is_none());
    assert_eq!(listed.reading, Some((1, 1)));
    state.take(Tell::Read);
    state.take(Tell::Said(Vec::new()));
    let read = state.build();
    assert!(read.text.is_none());
    assert_eq!(read.reading, None);
    assert_eq!(read.rows.lines(), first.rows.lines());
    assert_eq!(read.highlights.len(), first.highlights.len());
    // A line under the title changes it.
    state.take(Tell::Said(vec!["the project is no longer trusted".into()]));
    assert!(state.build().text.is_some());
}

#[test]
fn a_file_read_before_it_is_listed_goes_last_once() {
    let mut state = State::new("t".into());
    let entries: Arc<[Entry]> = vec![entry(b"a", Status::Modified)].into();
    state.take(Tell::Listed {
        entries,
        more: 0,
        cut: false,
    });
    for _ in 0..2 {
        state.take(Tell::File {
            reading: Arc::new(text_reading(b"b", vec![hunk(1, 1, &[b"x\n"], &[])])),
            carry: None,
        });
    }
    assert_eq!(
        state.order,
        [b"a".as_slice().into(), b"b".as_slice().into()]
    );
}

#[test]
fn the_next_text_waits_until_the_ui_took_the_last() {
    let (sent, texts) = mpsc::channel();
    let view = View::start("t".into(), move |text| sent.send(text).is_ok()).unwrap();
    view.tell(Tell::Said(vec!["one".into()]));
    texts.recv_timeout(Duration::from_secs(10)).unwrap();
    // Told twice while the first text waits: nothing is built yet.
    view.tell(Tell::Said(vec!["two".into()]));
    view.tell(Tell::Said(vec!["three".into()]));
    assert!(texts.recv_timeout(Duration::from_millis(200)).is_err());
    // Once taken, one text has what both said.
    view.taken();
    let text = texts.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(text.text.is_some_and(|text| text.is_changed()));
    assert!(texts.recv_timeout(Duration::from_millis(200)).is_err());
    view.taken();
    // Taken with nothing told since: no text.
    assert!(texts.recv_timeout(Duration::from_millis(200)).is_err());
}

#[test]
fn a_listing_cut_short_says_more_files_may_not_be_listed() {
    let mut state = State::new("t".into());
    let entries: Arc<[Entry]> = vec![entry(b"a", Status::Modified)].into();
    state.take(Tell::Listed {
        entries: Arc::clone(&entries),
        more: 0,
        cut: true,
    });
    state.build();
    assert!(
        state
            .text
            .to_string()
            .contains("more files may not be listed")
    );
    state.take(Tell::Listed {
        entries,
        more: 0,
        cut: false,
    });
    assert!(state.dirty);
    state.build();
    assert!(!state.text.to_string().contains("may not be listed"));
}
