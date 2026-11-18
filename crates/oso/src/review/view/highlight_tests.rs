//! The view highlighting its sides: what it opens and never opens, a report's runs in the
//! text, a paint that changes no text, the files on screen first, the cap on open documents,
//! closing, failures and the budget's bookkeeping.
//!
//! Must not depend on: threads or the syntax worker; a stand-in highlighter records what is
//! asked, and reports are given by hand.

use std::ops::Range;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use oso_core::buffer::Verdict;
use oso_core::review::Review;
use oso_core::rope::Rope;
use oso_core::syntax::{Highlight, Highlights};
use oso_syntax::{Failure, Language, MAX_BYTES, Outcome, Refused, Report};

use super::highlight::{Highlighter, MAX_OPEN};
use super::section::tests::{entry, hunk};
use super::*;
use crate::git::Id;
use crate::review::Content;
use crate::review::list::Status;

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Call {
    Open {
        key: u64,
        language: Language,
        verdict: Verdict,
        text: String,
        shown: Vec<Range<usize>>,
    },
    Show(u64, Vec<Range<usize>>),
    Close(u64),
}

#[derive(Clone, Default)]
pub(super) struct Recorder {
    pub(super) calls: Arc<Mutex<Vec<Call>>>,
    /// Opens past this many are refused as full.
    pub(super) room: Option<usize>,
    /// Every open asked for, refused ones too.
    pub(super) asked: Arc<Mutex<usize>>,
}

impl Recorder {
    pub(super) fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }

    pub(super) fn opened(&self) -> Vec<(u64, String)> {
        let calls = self.calls();
        let mut opened: Vec<(u64, String)> = Vec::new();
        for call in calls {
            match call {
                Call::Open { key, text, .. } => opened.push((key, text)),
                Call::Close(key) => opened.retain(|(open, _)| *open != key),
                Call::Show(..) => {}
            }
        }
        opened
    }
}

impl Highlighter for Recorder {
    fn open(
        &self,
        key: u64,
        language: Language,
        verdict: Verdict,
        text: Rope,
        shown: Vec<Range<usize>>,
    ) -> Result<(), Refused> {
        *self.asked.lock().unwrap() += 1;
        let mut calls = self.calls.lock().unwrap();
        let open = calls
            .iter()
            .filter(|call| matches!(call, Call::Open { .. }))
            .count();
        if self.room.is_some_and(|room| open >= room) {
            return Err(Refused::Full);
        }
        calls.push(Call::Open {
            key,
            language,
            verdict,
            text: text.to_string(),
            shown,
        });
        Ok(())
    }

    fn show(&self, key: u64, shown: Vec<Range<usize>>) {
        self.calls.lock().unwrap().push(Call::Show(key, shown));
    }

    fn close(&self, key: u64) {
        self.calls.lock().unwrap().push(Call::Close(key));
    }
}

pub(super) fn id(n: u8) -> Id {
    Id::parse(format!("{n:040x}").as_bytes()).unwrap()
}

/// A text reading of `path` changing line 1 from `old` to `new`, with blob ids.
pub(super) fn reading(path: &[u8], old: &[u8], new: &[u8], ids: (u8, u8)) -> Arc<Reading> {
    let mut entry = entry(path, Status::Modified);
    entry.old_id = id(ids.0);
    entry.new_id = id(ids.1);
    let hunks = vec![hunk(1, 1, &[old], &[new])];
    Arc::new(Reading {
        entry,
        content: Content::Text {
            shape: Arc::new(
                oso_core::review::Shape::new(oso_core::review::Base::Unknown, hunks).unwrap(),
            ),
            old: Arc::from(old),
            new: Arc::from(new),
            refined: Default::default(),
        },
    })
}

/// A view with `recorder` as its highlighter, the files of `readings` listed and read.
pub(super) fn state_with(recorder: &Recorder, readings: &[Arc<Reading>]) -> State {
    let mut state = State::new("t".into());
    state.take(Tell::Highlighter {
        highlighter: Some(Box::new(recorder.clone())),
        top: PathBuf::from("/top"),
    });
    let entries: Arc<[Entry]> = readings
        .iter()
        .map(|reading| reading.entry.clone())
        .collect();
    state.take(Tell::Listed {
        entries,
        more: 0,
        cut: false,
    });
    let review = Review::new();
    for reading in readings {
        let Content::Text { shape, .. } = &reading.content else {
            unreachable!()
        };
        let carry = review.carry(&reading.entry.path, Arc::clone(shape));
        state.take(Tell::File {
            reading: Arc::clone(reading),
            carry: Carrying::Here(carry),
        });
    }
    state.take(Tell::Read);
    state.build();
    state.schedule();
    state
}

/// The highlights of `text`, its first `len` bytes a keyword.
pub(super) fn keyword(text: &str, len: usize) -> Highlights {
    let mut highlights = Highlights::new(text.len());
    highlights.replace(0..len, [(len, Some(Highlight::Keyword))]);
    highlights
}

pub(super) fn report(key: u64, outcome: Outcome, done: bool) -> Report<u64> {
    Report {
        buffer: key,
        epoch: 0,
        stamp: 1,
        revision: 0,
        outcome,
        done,
    }
}

/// The highlight of the byte at `needle`'s first match in the last text, plus `skip`.
pub(super) fn at(state: &State, text: &Text, needle: &str, skip: usize) -> Option<Highlight> {
    let offset = state.text.to_string().find(needle).unwrap() + skip;
    text.highlights.at(offset)
}

#[test]
fn both_sides_of_an_ordinary_file_open_and_a_report_colours_the_lines_without_a_new_text() {
    let recorder = Recorder::default();
    let mut state = state_with(
        &recorder,
        &[reading(b"a.rs", b"fn a\n", b"let b\n", (1, 2))],
    );
    let calls = recorder.calls();
    let [
        Call::Open {
            key: old,
            language: Language::Rust,
            verdict: Verdict::Ordinary,
            text: old_text,
            ..
        },
        Call::Open {
            key: new,
            text: new_text,
            ..
        },
    ] = calls.as_slice()
    else {
        panic!("{calls:?}");
    };
    assert_ne!(old, new, "each text its own key");
    assert_eq!(
        (old_text.as_str(), new_text.as_str()),
        ("fn a\n", "let b\n")
    );
    let new = *new;
    let before = state.build();
    assert_eq!(at(&state, &before, "+let", 1), Some(Highlight::DiffAdded));
    state.take(Tell::Syntax(report(
        new,
        Outcome::Highlights(keyword("let b\n", 3)),
        true,
    )));
    let painted = state.build();
    assert!(painted.text.is_none(), "a paint changes no text");
    assert_eq!(at(&state, &painted, "+let", 0), Some(Highlight::DiffAdded));
    assert_eq!(at(&state, &painted, "+let", 1), Some(Highlight::Keyword));
    assert_eq!(at(&state, &painted, "+let", 5), None, "plain once whole");
    assert_eq!(at(&state, &painted, "-fn", 1), Some(Highlight::DiffRemoved));
    // Done: its document is closed, and a late report of its key is dropped.
    assert!(recorder.calls().contains(&Call::Close(new)));
    state.take(Tell::Syntax(report(
        new,
        Outcome::Highlights(Highlights::new(6)),
        true,
    )));
    let late = state.build();
    assert_eq!(at(&state, &late, "+let", 1), Some(Highlight::Keyword));
}

#[test]
fn a_side_partly_done_colours_its_lines_on_screen_only_and_keeps_its_diff_face_elsewhere() {
    let recorder = Recorder::default();
    let mut state = state_with(
        &recorder,
        &[reading(b"a.rs", b"fn a\n", b"let b\n", (1, 2))],
    );
    let (old, new) = (1, 2);
    state.take(Tell::Syntax(report(
        new,
        Outcome::Highlights(keyword("let b\n", 3)),
        true,
    )));
    // Partly done, off screen: nothing is projected yet.
    let partly = |state: &mut State| {
        state.take(Tell::Syntax(report(
            old,
            Outcome::Highlights(keyword("fn a\n", 2)),
            false,
        )));
    };
    partly(&mut state);
    let off = state.build();
    assert_eq!(at(&state, &off, "-fn", 1), Some(Highlight::DiffRemoved));
    let progress = |shown, whole| Some(super::Progress { shown, whole });
    assert_eq!(off.highlighted, progress(false, false), "no screen known");
    // On screen, its lines are.
    let len = state.text.len();
    assert!(!state.take(Tell::Shown(std::iter::once(0..len).collect())));
    partly(&mut state);
    let partial = state.build();
    assert_eq!(
        partial.highlighted,
        progress(false, false),
        "the old side not done"
    );
    assert_eq!(at(&state, &partial, "-fn", 1), Some(Highlight::Keyword));
    assert_eq!(
        at(&state, &partial, "-fn", 4),
        Some(Highlight::DiffRemoved),
        "not whole yet"
    );
    assert!(!recorder.calls().contains(&Call::Close(old)));
    state.take(Tell::Syntax(report(
        old,
        Outcome::Highlights(keyword("fn a\n", 2)),
        true,
    )));
    assert_eq!(state.build().highlighted, progress(true, true));
}

#[test]
fn a_file_that_may_hold_secrets_too_large_or_in_no_language_is_never_opened() {
    let recorder = Recorder::default();
    let big = vec![b'a'; MAX_BYTES + 1];
    let renamed = reading(b"src/b.rs", b"x\n", b"y\n", (3, 4));
    let mut entry = renamed.entry.clone();
    entry.old_path = Some(b".env.local".to_vec().into());
    let Content::Text {
        shape, old, new, ..
    } = &renamed.content
    else {
        unreachable!()
    };
    let renamed = Reading {
        entry,
        content: Content::Text {
            shape: Arc::clone(shape),
            old: Arc::clone(old),
            new: Arc::clone(new),
            refined: Default::default(),
        },
    };
    let readings = [
        reading(b".ssh/id_rsa.rs", b"x\n", b"y\n", (1, 2)),
        Arc::new(renamed),
        reading(b"notes.unknown", b"x\n", b"y\n", (5, 6)),
        reading(b"big.rs", &big, b"y\n", (7, 8)),
    ];
    let state = state_with(&recorder, &readings);
    let opened = recorder.opened();
    assert_eq!(opened.len(), 1, "{opened:?}");
    assert_eq!(opened[0].1, "y\n", "only the new side of the large file");
    drop(state);
}

#[test]
fn at_most_the_cap_is_open_the_files_on_screen_first_and_one_done_lets_the_next_open() {
    let recorder = Recorder::default();
    let readings: Vec<Arc<Reading>> = (0..MAX_OPEN as u8)
        .map(|n| {
            let path = format!("f{n}.rs");
            reading(path.as_bytes(), b"a\n", b"b\n", (2 * n + 1, 2 * n + 2))
        })
        .collect();
    let mut state = State::new("t".into());
    let entries: Arc<[Entry]> = readings.iter().map(|r| r.entry.clone()).collect();
    state.take(Tell::Listed {
        entries,
        more: 0,
        cut: false,
    });
    let review = Review::new();
    for reading in &readings {
        let Content::Text { shape, .. } = &reading.content else {
            unreachable!()
        };
        let carry = review.carry(&reading.entry.path, Arc::clone(shape));
        state.take(Tell::File {
            reading: Arc::clone(reading),
            carry: Carrying::Here(carry),
        });
    }
    state.build();
    // The last file's lines are on screen.
    let text = state.text.to_string();
    let last = text.find("modified  f7.rs").unwrap();
    state.take(Tell::Shown(std::iter::once(last..text.len()).collect()));
    state.take(Tell::Highlighter {
        highlighter: Some(Box::new(recorder.clone())),
        top: PathBuf::from("/top"),
    });
    state.schedule();
    let calls = recorder.calls();
    let Some(Call::Open { key, shown, .. }) = calls.first() else {
        panic!("{calls:?}");
    };
    assert_eq!(*key, 1);
    assert_eq!(
        shown.as_slice(),
        std::slice::from_ref(&(0..1)),
        "the line on screen"
    );
    assert_eq!(recorder.opened().len(), MAX_OPEN);
    let first: Vec<u64> = recorder.opened().iter().map(|(key, _)| *key).collect();
    assert_eq!(first, (1..=MAX_OPEN as u64).collect::<Vec<_>>());
    // The on-screen file's two sides went first: f7 holds keys 1 and 2.
    state.take(Tell::Syntax(report(
        1,
        Outcome::Highlights(keyword("a\n", 1)),
        true,
    )));
    state.schedule();
    assert_eq!(recorder.opened().len(), MAX_OPEN, "the next took its place");
}

#[test]
fn a_new_reading_of_the_same_old_blob_keeps_its_runs_and_a_new_text_closes_and_reopens() {
    let recorder = Recorder::default();
    let first = reading(b"a.rs", b"fn a\n", b"let b\n", (1, 2));
    let mut state = state_with(&recorder, std::slice::from_ref(&first));
    state.take(Tell::Syntax(report(
        1,
        Outcome::Highlights(keyword("fn a\n", 2)),
        true,
    )));
    state.build();
    let second = reading(b"a.rs", b"fn a\n", b"let c\n", (1, 3));
    let Content::Text { shape, .. } = &second.content else {
        unreachable!()
    };
    let carry = Review::new().carry(b"a.rs", Arc::clone(shape));
    state.take(Tell::File {
        reading: Arc::clone(&second),
        carry: Carrying::Here(carry),
    });
    let shown = state.build();
    state.schedule();
    assert_eq!(
        at(&state, &shown, "-fn", 1),
        Some(Highlight::Keyword),
        "kept"
    );
    assert!(
        recorder.calls().contains(&Call::Close(2)),
        "the new side's old text"
    );
    let opened = recorder.opened();
    assert_eq!(
        opened,
        [(3, "let c\n".to_owned())],
        "only the new text opens"
    );
}

#[test]
fn a_side_that_failed_stays_plain_and_a_long_line_tries_its_next_text() {
    let recorder = Recorder::default();
    let first = reading(b"a.rs", b"fn a\n", b"let b\n", (1, 2));
    let mut state = state_with(&recorder, std::slice::from_ref(&first));
    state.take(Tell::Syntax(report(
        1,
        Outcome::Failed(Failure::TooSlow),
        false,
    )));
    state.take(Tell::Syntax(report(
        2,
        Outcome::Failed(Failure::LongLine),
        false,
    )));
    state.schedule();
    assert!(recorder.opened().is_empty(), "{:?}", recorder.calls());
    // New texts of both sides: the slow one stays plain, the long line is tried again.
    let second = reading(b"a.rs", b"fn z\n", b"let c\n", (4, 5));
    let Content::Text { shape, .. } = &second.content else {
        unreachable!()
    };
    let carry = Review::new().carry(b"a.rs", Arc::clone(shape));
    state.take(Tell::File {
        reading: second,
        carry: Carrying::Here(carry),
    });
    state.build();
    state.schedule();
    let opened = recorder.opened();
    assert_eq!(opened.len(), 1, "{opened:?}");
    assert_eq!(opened[0].1, "let c\n");
}

#[test]
fn a_full_schedule_is_tried_again_and_turning_highlighting_off_closes_everything() {
    let recorder = Recorder {
        room: Some(1),
        ..Recorder::default()
    };
    let mut state = state_with(
        &recorder,
        &[reading(b"a.rs", b"fn a\n", b"let b\n", (1, 2))],
    );
    assert_eq!(recorder.opened().len(), 1);
    state.schedule();
    assert_eq!(recorder.opened().len(), 1, "full, still waiting");
    let len = state.text.len();
    state.take(Tell::Shown(std::iter::once(0..len).collect()));
    state.take(Tell::Syntax(report(
        1,
        Outcome::Highlights(keyword("fn a\n", 2)),
        false,
    )));
    let shown = state.build();
    assert_eq!(at(&state, &shown, "-fn", 1), Some(Highlight::Keyword));
    state.take(Tell::Highlighter {
        highlighter: None,
        top: PathBuf::new(),
    });
    assert!(recorder.opened().is_empty(), "{:?}", recorder.calls());
    let off = state.build();
    assert_eq!(at(&state, &off, "-fn", 1), Some(Highlight::DiffRemoved));
    // On again: both sides open again, and a text laid out anew closes neither.
    let again = Recorder::default();
    state.take(Tell::Highlighter {
        highlighter: Some(Box::new(again.clone())),
        top: PathBuf::from("/top"),
    });
    state.schedule();
    assert_eq!(again.opened().len(), 2);
    state.dirty = true;
    state.build();
    assert_eq!(again.opened().len(), 2, "{:?}", again.calls());
}

#[test]
fn a_file_that_leaves_the_review_closes_its_documents() {
    let recorder = Recorder::default();
    let mut state = state_with(
        &recorder,
        &[reading(b"a.rs", b"fn a\n", b"let b\n", (1, 2))],
    );
    assert_eq!(recorder.opened().len(), 2);
    state.take(Tell::Listed {
        entries: Arc::from(Vec::new()),
        more: 0,
        cut: false,
    });
    assert!(recorder.opened().is_empty(), "{:?}", recorder.calls());
    let calls = recorder.calls();
    assert!(!calls.iter().any(|call| matches!(call, Call::Show(..))));
}
