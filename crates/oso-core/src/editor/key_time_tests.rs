//! Tests of a key's time on the UI thread (ADR 0082, ADR 0091): oso-core's other tests give a
//! key no time limit, so that what one key finds does not depend on the machine's speed; these
//! give it none at all, as on a busy machine, and the worker goes on with what the key left.
//! A regexp keeps to the time, as it can read far past each place it looks; a literal string
//! keeps to its budget of bytes alone.
//!
//! Must not depend on: `command` or `keymap`; these run on the editor alone.

use std::time::Duration;

use super::Editor;
use super::cursors::{CursorError, Toward};
use crate::buffer::Buffer;

fn editor(text: &str) -> Editor {
    let mut editor = Editor::with_buffer(Buffer::from_text("notes.txt", text));
    editor.set_migemo(false);
    editor
}

/// Runs the worker's job in place, as the host's thread would, and takes in its reports.
fn drain(editor: &mut Editor) {
    while let Some(job) = editor.take_isearch_job() {
        let mut reports = Vec::new();
        job.run(|report| reports.push(report));
        for report in reports {
            editor.isearch_reported(report);
        }
    }
}

/// The cursors' regions, as `(mark, cursor)`, in order, and which is primary.
fn regions(editor: &Editor) -> (Vec<(usize, usize)>, usize) {
    let set = &editor.window().selections;
    let regions = set
        .iter()
        .map(|s| (s.mark.unwrap_or(s.cursor), s.cursor))
        .collect();
    (regions, set.primary_index())
}

#[test]
fn an_isearch_key_out_of_time_leaves_its_match_to_the_worker() {
    let mut editor = editor("one foo two foo three foo");
    editor.set_key_time(Some(Duration::ZERO));
    editor.isearch_begin(false, true);
    editor.isearch_add("fo+");
    assert!(editor.isearch_current_match().is_none());
    assert_eq!(regions(&editor), (vec![(0, 0)], 0), "the cursor stays");
    drain(&mut editor);
    let (_, current, total, _) = editor.isearch_current_match().expect("found");
    assert_eq!((current, total), (4..7, Some(3)), "the first, counted");
}

#[test]
fn an_occurrence_past_the_keys_time_comes_from_the_worker() {
    let mut editor = editor("a1 b a22 a3");
    editor.isearch_begin(false, true);
    editor.isearch_add(r"a\d+");
    drain(&mut editor);
    editor.set_key_time(Some(Duration::ZERO));
    editor.isearch_mark(Some(Toward::Next)).unwrap();
    assert_eq!(
        regions(&editor),
        (vec![(0, 2)], 0),
        "the match is the region at once"
    );
    drain(&mut editor);
    assert_eq!(regions(&editor), (vec![(0, 2), (5, 8)], 1));
}

#[test]
fn every_match_waits_for_the_worker_to_find_the_match_and_count() {
    let mut editor = editor("ab ab");
    editor.set_key_time(Some(Duration::ZERO));
    editor.isearch_begin(false, true);
    editor.isearch_add("ab");
    assert_eq!(
        editor.isearch_mark(None),
        Err(CursorError::NoCurrentMatch),
        "the worker still looks for it"
    );
    // The worker reports the match before the count.
    let job = editor.take_isearch_job().expect("the worker's");
    let mut reports = Vec::new();
    job.run(|report| reports.push(report));
    let mut reports = reports.into_iter();
    while editor.isearch_current_match().is_none() {
        editor.isearch_reported(reports.next().expect("the worker finds the match"));
    }
    assert_eq!(editor.isearch_mark(None), Err(CursorError::Counting));
    for report in reports {
        editor.isearch_reported(report);
    }
    editor.isearch_mark(None).unwrap();
    drain(&mut editor);
    assert_eq!(regions(&editor), (vec![(0, 2), (3, 5)], 0));
}
