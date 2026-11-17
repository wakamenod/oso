//! The view's highlighting at its limits and from elsewhere: a side past the budget, a full
//! schedule, a file the text cut short, a withheld reading, a stale report, what windows show,
//! the view dropped, the panes, and an open buffer's runs taken for a new side.
//!
//! Must not depend on: threads or the syntax worker; a stand-in highlighter records what is
//! asked, and reports are given by hand.

use std::path::PathBuf;
use std::sync::Arc;

use oso_core::syntax::{Highlight, Highlights};
use oso_syntax::{Language, Outcome};

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

/// The highlight of the byte at `needle`'s first match in pane `side`'s text, plus `skip`.
fn pane_at(text: &str, highlights: &Highlights, needle: &str, skip: usize) -> Option<Highlight> {
    highlights.at(text.find(needle).unwrap() + skip)
}

#[test]
fn the_panes_take_each_sides_runs_laid_out_and_as_a_paint() {
    let recorder = Recorder::default();
    let mut state = state_with(
        &recorder,
        &[reading(b"a.rs", b"fn a\n", b"let b\n", (1, 2))],
    );
    // Highlighted before the panes are built: their first texts hold the runs.
    state.take(Tell::Syntax(report(
        1,
        Outcome::Highlights(keyword("fn a\n", 2)),
        true,
    )));
    state.build();
    state.take(Tell::Panes(true));
    let text = state.build();
    let panes = text.panes.expect("the panes");
    let [old, new] = *panes;
    let old_text = state.panes.texts[0].to_string();
    let new_text = state.panes.texts[1].to_string();
    assert_eq!(
        pane_at(&old_text, &old.2, "-fn", 1),
        Some(Highlight::Keyword)
    );
    assert_eq!(
        pane_at(&new_text, &new.2, "+let", 1),
        Some(Highlight::DiffAdded)
    );
    // The new side's report: a paint of both panes, with no text.
    state.take(Tell::Syntax(report(
        2,
        Outcome::Highlights(keyword("let b\n", 3)),
        true,
    )));
    let painted = state.build();
    assert!(painted.panes.is_none() && painted.text.is_none());
    let [old, new] = *painted.pane_paint.expect("a paint of the panes");
    assert_eq!(
        pane_at(&new_text, &new.1, "+let", 1),
        Some(Highlight::Keyword)
    );
    assert_eq!(
        pane_at(&new_text, &new.1, "+let", 0),
        Some(Highlight::DiffAdded)
    );
    assert_eq!(
        pane_at(&old_text, &old.1, "-fn", 1),
        Some(Highlight::Keyword)
    );
    // With nothing changed, no paint.
    assert!(state.build().pane_paint.is_none());
}

/// An open buffer of `path` holding `text` at `revision`, its first `keyword_len` bytes a
/// keyword, in Rust.
fn buffer(path: &[u8], (text, revision): (&str, u64), keyword_len: usize, whole: bool) -> Tell {
    Tell::Buffer(Box::new(BufferSide {
        path: path.into(),
        revision,
        text: oso_core::rope::Rope::from(text),
        highlights: keyword(text, keyword_len),
        whole,
        language: Some(Language::Rust),
    }))
}

/// Offers `buffer` to `state`; whether its batch changed what the text shows.
fn offer(state: &mut State, buffer: Tell) -> bool {
    assert!(!state.take(buffer), "an offer waits for its batch");
    state.schedule()
}

#[test]
fn an_open_buffers_whole_runs_are_its_files_new_sides_and_its_document_closes() {
    let recorder = Recorder::default();
    let mut state = state_with(
        &recorder,
        &[reading(b"a.rs", b"fn a\n", b"let b\n", (1, 2))],
    );
    assert_eq!(recorder.opened().len(), 2, "both sides open first");
    // Another text, or the same in another language: not the side's.
    assert!(!offer(&mut state, buffer(b"a.rs", ("let c\n", 6), 3, true)));
    let Tell::Buffer(mut other) = buffer(b"a.rs", ("let b\n", 7), 3, true) else {
        unreachable!()
    };
    other.language = Some(Language::Markdown);
    assert!(!offer(&mut state, Tell::Buffer(other)));
    // The side's text, whole: its runs, and the new side's document closes.
    assert!(offer(&mut state, buffer(b"a.rs", ("let b\n", 8), 3, true)));
    let text = state.build();
    assert_eq!(at(&state, &text, "+let", 1), Some(Highlight::Keyword));
    assert_eq!(at(&state, &text, "+let", 4), None, "whole: plain");
    let opened = recorder.opened();
    assert_eq!(opened.len(), 1, "{opened:?}");
    assert_eq!(opened[0].1, "fn a\n", "the old side's only");
    // Whole already: the next offer changes nothing.
    assert!(!offer(&mut state, buffer(b"a.rs", ("let b\n", 8), 3, true)));
}

#[test]
fn a_buffer_not_whole_colours_the_lines_on_screen_and_a_new_reading_tries_it_again() {
    let recorder = Recorder::default();
    let mut state = state_with(
        &recorder,
        &[reading(b"a.rs", b"fn a\n", b"let b\n", (1, 2))],
    );
    // Nothing on screen: a buffer partly highlighted gives nothing yet; once on screen, the
    // buffer kept gives its lines there.
    assert!(!offer(
        &mut state,
        buffer(b"a.rs", ("let b\n", 7), 3, false)
    ));
    all_shown(&mut state);
    assert!(state.schedule(), "on screen: tried again");
    let text = state.build();
    assert_eq!(at(&state, &text, "+let", 1), Some(Highlight::Keyword));
    assert_eq!(
        at(&state, &text, "+let", 4),
        Some(Highlight::DiffAdded),
        "not whole: the diff face"
    );
    assert!(
        recorder.opened().iter().any(|(_, text)| text == "let b\n"),
        "its document stays"
    );
    // Offered whole, then a new reading of the same text: the buffer kept is tried again.
    assert!(offer(&mut state, buffer(b"a.rs", ("let b\n", 9), 3, true)));
    let again = reading(b"a.rs", b"fn a\n", b"let b\n", (1, 3));
    state.take(Tell::File {
        reading: again,
        carry: Carrying::Nothing,
    });
    let text = state.build();
    assert_eq!(at(&state, &text, "+let", 1), Some(Highlight::Keyword));
    assert_eq!(at(&state, &text, "+let", 4), None);
}

#[test]
fn no_buffer_is_kept_or_taken_while_highlighting_is_off_and_a_withheld_reading_takes_none() {
    let recorder = Recorder::default();
    let mut state = state_with(
        &recorder,
        &[reading(b"a.rs", b"fn a\n", b"let b\n", (1, 2))],
    );
    state.take(Tell::Highlighter {
        highlighter: None,
        top: PathBuf::new(),
    });
    assert!(!offer(&mut state, buffer(b"a.rs", ("let b\n", 7), 3, true)));
    // A new reading the buffer's text is: still nothing while off.
    let again = reading(b"a.rs", b"fn a\n", b"let b\n", (1, 3));
    state.take(Tell::File {
        reading: again,
        carry: Carrying::Nothing,
    });
    let text = state.build();
    assert_eq!(at(&state, &text, "+let", 1), Some(Highlight::DiffAdded));
    // On again, a withheld reading of the file takes none of an offer.
    state.take(Tell::Highlighter {
        highlighter: Some(Box::new(recorder.clone())),
        top: PathBuf::from("/top"),
    });
    let mut entry = entry(b"a.rs", Status::Modified);
    entry.new_id = id(9);
    state.take(Tell::File {
        reading: Arc::new(Reading {
            entry,
            content: Content::Withheld,
        }),
        carry: Carrying::Nothing,
    });
    state.build();
    assert!(!offer(
        &mut state,
        buffer(b"a.rs", ("let b\n", 10), 3, true)
    ));
}
