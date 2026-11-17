//! The view's highlighting at its limits: a side past the budget, a full schedule, a file the
//! text cut short, a withheld reading, a stale report, what windows show, and the view dropped.
//!
//! Must not depend on: threads or the syntax worker; a stand-in highlighter records what is
//! asked, and reports are given by hand.

use std::path::PathBuf;
use std::sync::Arc;

use oso_core::syntax::{Highlight, Highlights};
use oso_syntax::Outcome;

use super::highlight_tests::{Recorder, at, id, keyword, reading, report, state_with};
use super::section::tests::{entry, hunk};
use super::*;
use crate::review::Content;
use crate::review::list::Status;

/// A text reading of `path` adding `added` lines, as a new file's.
fn added(path: &[u8], added: &[&[u8]], n: u8) -> Arc<Reading> {
    let mut entry = entry(path, Status::Added);
    entry.new_id = id(n);
    let new: Vec<u8> = added.concat();
    let hunks = vec![hunk(1, 1, &[], added)];
    Arc::new(Reading {
        entry,
        content: Content::Text {
            shape: Arc::new(
                oso_core::review::Shape::new(oso_core::review::Base::Unknown, hunks).unwrap(),
            ),
            old: Arc::from(&b""[..]),
            new: Arc::from(new),
        },
    })
}

/// Tells `state` that its whole text shows.
fn all_shown(state: &mut State) {
    let len = state.text.len();
    state.take(Tell::Shown(std::iter::once(0..len).collect()));
}

#[test]
fn a_side_on_screen_past_the_budget_is_parsed_once_until_other_files_show() {
    let recorder = Recorder::default();
    let lines: [&[u8]; 4] = [b"fn a\n", b"fn b\n", b"fn c\n", b"fn d\n"];
    let mut state = State::new("t".into());
    state.highlighting.set_budget(3);
    let readings = [added(b"a.rs", &lines, 1)];
    let entries: Arc<[Entry]> = readings.iter().map(|r| r.entry.clone()).collect();
    state.take(Tell::Listed {
        entries,
        more: 0,
        cut: false,
    });
    state.take(Tell::File {
        reading: Arc::clone(&readings[0]),
        carry: Carrying::Nothing,
    });
    state.build();
    all_shown(&mut state);
    state.take(Tell::Highlighter {
        highlighter: Some(Box::new(recorder.clone())),
        top: PathBuf::from("/top"),
    });
    state.schedule();
    let key = recorder.opened()[0].0;
    // Four lines, each a keyword then plain: past a budget of 3.
    let mut highlights = Highlights::new(20);
    for line in 0..4 {
        highlights.replace(line * 5..line * 5 + 2, [(2, Some(Highlight::Keyword))]);
    }
    state.take(Tell::Syntax(report(
        key,
        Outcome::Highlights(highlights),
        true,
    )));
    let shown = state.build();
    assert_eq!(
        at(&state, &shown, "+fn a", 1),
        Some(Highlight::DiffAdded),
        "kept none"
    );
    for _ in 0..3 {
        state.schedule();
        state.build();
    }
    assert_eq!(*recorder.asked.lock().unwrap(), 1, "opened once");
    // Another screen, with no file on it: it may be tried again once it shows.
    state.take(Tell::Shown(Vec::new()));
    all_shown(&mut state);
    state.schedule();
    assert_eq!(*recorder.asked.lock().unwrap(), 2);
}

#[test]
fn a_view_dropped_closes_every_document_it_opened() {
    let recorder = Recorder::default();
    let state = state_with(
        &recorder,
        &[
            reading(b"a.rs", b"fn a\n", b"let b\n", (1, 2)),
            reading(b"b.rs", b"fn c\n", b"let d\n", (3, 4)),
        ],
    );
    assert_eq!(recorder.opened().len(), 4);
    drop(state);
    assert!(recorder.opened().is_empty(), "{:?}", recorder.calls());
}

#[test]
fn a_full_schedule_asks_once_a_batch_and_decodes_nothing_more() {
    let recorder = Recorder {
        room: Some(0),
        ..Recorder::default()
    };
    let readings: Vec<Arc<Reading>> = (0..6u8)
        .map(|n| {
            reading(
                format!("f{n}.rs").as_bytes(),
                b"a\n",
                b"b\n",
                (2 * n + 1, 2 * n + 2),
            )
        })
        .collect();
    let mut state = state_with(&recorder, &readings);
    assert_eq!(*recorder.asked.lock().unwrap(), 1);
    state.schedule();
    assert_eq!(*recorder.asked.lock().unwrap(), 2, "one more a batch");
}

#[test]
fn a_file_the_text_cut_short_and_a_withheld_reading_open_nothing() {
    let recorder = Recorder::default();
    let mut state = State::new("t".into());
    state.max_text = 1;
    let readings = [reading(b"a.rs", b"fn a\n", b"let b\n", (1, 2))];
    let entries: Arc<[Entry]> = readings.iter().map(|r| r.entry.clone()).collect();
    state.take(Tell::Listed {
        entries,
        more: 0,
        cut: false,
    });
    state.take(Tell::Highlighter {
        highlighter: Some(Box::new(recorder.clone())),
        top: PathBuf::from("/top"),
    });
    state.take(Tell::File {
        reading: Arc::clone(&readings[0]),
        carry: Carrying::Nothing,
    });
    state.build();
    state.schedule();
    assert!(recorder.calls().is_empty(), "its section shows no line");
    // Room again: it opens; then withheld, it closes at once, with no text built.
    state.max_text = MAX_TEXT;
    state.dirty = true;
    state.build();
    state.schedule();
    assert_eq!(recorder.opened().len(), 2);
    let mut entry = readings[0].entry.clone();
    entry.new_id = id(9);
    state.take(Tell::File {
        reading: Arc::new(Reading {
            entry,
            content: Content::Withheld,
        }),
        carry: Carrying::Nothing,
    });
    assert!(recorder.opened().is_empty(), "{:?}", recorder.calls());
}

#[test]
fn what_windows_show_changes_no_text_and_a_stale_report_is_dropped() {
    let recorder = Recorder::default();
    let mut state = state_with(
        &recorder,
        &[reading(b"a.rs", b"fn a\n", b"let b\n", (1, 2))],
    );
    let len = state.text.len();
    assert!(!state.take(Tell::Shown(std::iter::once(0..len).collect())));
    let mut late = report(2, Outcome::Highlights(keyword("let b\n", 3)), false);
    let newer = report(2, Outcome::Highlights(Highlights::new(6)), false);
    late.stamp = 0;
    state.take(Tell::Syntax(newer));
    state.take(Tell::Syntax(late));
    let text = state.build();
    assert_eq!(
        at(&state, &text, "+let", 1),
        Some(Highlight::DiffAdded),
        "the older report did not count"
    );
}
