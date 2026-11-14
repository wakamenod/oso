//! Tests of the keys that add cursors from isearch (ADR 0091): the search's match as the
//! region, on whole clusters and in its direction, its string and case, and what waits for
//! its count.
//!
//! Must not depend on: `command` or `keymap`; these run on the editor alone.

use super::tests::{at, cursors, drain, editor, regions};
use super::{CursorError, Toward};
use crate::editor::Editor;
use crate::selection::MAX_CURSORS;

/// Starts a forward search for `string`, and lets the worker count.
fn search(editor: &mut Editor, string: &str) {
    editor.isearch_begin(false, false);
    editor.isearch_add(string);
    drain(editor);
}

#[test]
fn a_search_ends_with_its_match_as_the_region_and_adds_the_next_with_its_case() {
    let mut editor = editor("ab AB abc ab");
    search(&mut editor, "ab");
    editor.isearch_mark(Some(Toward::Next)).unwrap();
    assert!(editor.isearch().is_none());
    assert_eq!(
        regions(&editor),
        (vec![(0, 2), (3, 5)], 1),
        "smart case folds"
    );
    editor.mark_like_this(Toward::Next, 1).unwrap();
    assert_eq!(
        regions(&editor).0,
        [(0, 2), (3, 5), (6, 8)],
        "not only whole words"
    );
}

#[test]
fn a_failing_search_has_no_match_to_add_cursors_from_and_goes_on() {
    let mut editor = editor("ab ab");
    search(&mut editor, "abx");
    assert_eq!(
        editor.isearch_mark(Some(Toward::Next)),
        Err(CursorError::NoCurrentMatch)
    );
    assert!(editor.isearch().is_some());
}

#[test]
fn every_match_of_a_search_gets_a_cursor_unless_it_counted_too_many() {
    let mut editor = editor("ab x ab x ab");
    search(&mut editor, "ab");
    editor.isearch_mark(None).unwrap();
    assert!(editor.isearch().is_none());
    assert_eq!(regions(&editor), (vec![(0, 2), (5, 7), (10, 12)], 0));

    let text = "a ".repeat(MAX_CURSORS + 1);
    let mut editor = self::editor(&text);
    search(&mut editor, "a");
    assert_eq!(
        editor.isearch_mark(None),
        Err(CursorError::TooManyMatches(MAX_CURSORS + 1))
    );
    assert!(editor.isearch().is_some(), "the search goes on");
}

#[test]
fn a_match_from_a_search_is_taken_on_whole_clusters_in_the_searchs_direction() {
    // `e` matches the start of `e` + U+0301, which is one cluster.
    let mut editor = editor("x e\u{301} e");
    editor.isearch_begin(false, false);
    editor.isearch_add("e");
    drain(&mut editor);
    editor.isearch_mark(Some(Toward::Next)).unwrap();
    assert_eq!(regions(&editor), (vec![(2, 5), (6, 7)], 1));

    let mut editor = self::editor("ab x ab x ab");
    at(&mut editor, &[12]);
    editor.isearch_begin(true, false);
    editor.isearch_add("ab");
    drain(&mut editor);
    editor.isearch_mark(Some(Toward::Previous)).unwrap();
    assert_eq!(
        regions(&editor),
        (vec![(7, 5), (12, 10)], 0),
        "backward, as the search"
    );
}

#[test]
fn every_match_waits_for_the_search_to_count_them() {
    // The key finds the match, as a key with no time limit does in oso-core's tests; one out of
    // time is in `key_time_tests`.
    let mut editor = editor("ab ab");
    editor.isearch_begin(false, false);
    editor.isearch_add("ab");
    assert_eq!(editor.isearch_mark(None), Err(CursorError::Counting));
    assert!(editor.isearch().is_some(), "the search goes on");
    drain(&mut editor);
    editor.isearch_mark(None).unwrap();
    assert_eq!(cursors(&editor), [2, 5]);
}

#[test]
fn every_match_keeps_the_searchs_own_inside_a_cluster_as_the_primary() {
    // `e` matches the start of `e` + U+0301, which is no occurrence of its own: the search's
    // match, widened to the cluster, stays the primary.
    let mut editor = editor("x e\u{301} e");
    editor.isearch_begin(false, false);
    editor.isearch_add("e");
    drain(&mut editor);
    editor.isearch_mark(None).unwrap();
    assert_eq!(regions(&editor), (vec![(2, 5), (6, 7)], 0));
}

#[test]
fn a_regexp_search_adds_cursors_on_its_matches_and_never_an_empty_one() {
    let mut editor = editor("a1 b a22 a3");
    editor.isearch_begin(false, true);
    editor.isearch_add(r"a\d+");
    drain(&mut editor);
    editor.isearch_mark(Some(Toward::Next)).unwrap();
    assert_eq!(regions(&editor), (vec![(0, 2), (5, 8)], 1));
    editor.mark_like_this(Toward::Next, 1).unwrap();
    assert_eq!(regions(&editor).0, [(0, 2), (5, 8), (9, 11)]);
    // `b*` matches empty almost everywhere, and a cursor goes only on `b`.
    let mut editor = self::editor("b xb");
    editor.isearch_begin(false, true);
    editor.isearch_add("b*");
    drain(&mut editor);
    editor.isearch_mark(Some(Toward::Next)).unwrap();
    assert_eq!(regions(&editor), (vec![(0, 1), (3, 4)], 1));
}
