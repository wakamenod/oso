//! A review's two panes on the editor (ADR 0122, step 11): putting their texts in, switching
//! a window between the unified text and the panes with the place carried, the areas a review
//! shows in, filler rows skipped, and the panes dropped once nothing shows them.
//!
//! Must not depend on: `command` or `keymap`, or the host's threads or files.

use std::sync::Arc;

use super::{ReviewArea, ReviewLayout};
use crate::display_map::TabWidth;
use crate::editor::{Editor, NoPair, ReviewId};
use crate::frame::{FrameSize, Way};
use crate::load::{Reloaded, decode_all};
use crate::project::ProjectId;
use crate::review::{CommentId, RowKind, Rows, Section, Side};
use crate::selection::SelectionSet;
use crate::syntax::Highlights;
use crate::window::WindowId;

const PATH: &str = "a.rs";

fn lines(hunk: usize, side: Side, first: u32) -> RowKind {
    RowKind::Lines { hunk, side, first }
}

fn label() -> RowKind {
    RowKind::Label {
        comment: CommentId::of(1),
    }
}

fn card() -> RowKind {
    RowKind::Card {
        comment: CommentId::of(1),
    }
}

/// The rows of one file's section of `runs`.
fn rows(runs: &[(RowKind, usize)]) -> Rows {
    let mut section = Section::new(PATH.as_bytes().into());
    for &(kind, lines) in runs {
        section.push(kind, lines);
    }
    Rows::new(vec![Arc::new(section)])
}

/// A hunk changing old lines 2 and 3 into new lines 2 to 4, with a card on new line 2, as the
/// unified text shows it.
fn unified() -> (&'static str, Rows) {
    let text = "modified  a.rs\n@@ -2,2 +2,3 @@\n-old two\n-old three\n+new two\n    You  #1\n    on two\n+new three\n+new four\n";
    let rows = rows(&[
        (RowKind::File, 1),
        (RowKind::Hunk { hunk: 0 }, 1),
        (lines(0, Side::Old, 2), 2),
        (lines(0, Side::New, 2), 1),
        (label(), 1),
        (card(), 1),
        (lines(0, Side::New, 3), 2),
    ]);
    (text, rows)
}

/// The same as two panes, lined up by filler rows: the card's rows across the old pane's.
fn panes() -> ((&'static str, Rows), (&'static str, Rows)) {
    let old = "modified  a.rs\n@@ -2,2 +2,3 @@\n-old two\n\n\n-old three\n\n";
    let new = "modified  a.rs\n@@ -2,2 +2,3 @@\n+new two\n    You  #1\n    on two\n+new three\n+new four\n";
    let old_rows = rows(&[
        (RowKind::File, 1),
        (RowKind::Hunk { hunk: 0 }, 1),
        (lines(0, Side::Old, 2), 1),
        (RowKind::Filler, 2),
        (lines(0, Side::Old, 3), 1),
        (RowKind::Filler, 1),
    ]);
    let new_rows = rows(&[
        (RowKind::File, 1),
        (RowKind::Hunk { hunk: 0 }, 1),
        (lines(0, Side::New, 2), 1),
        (label(), 1),
        (card(), 1),
        (lines(0, Side::New, 3), 2),
    ]);
    ((old, old_rows), (new, new_rows))
}

fn loaded(text: &str) -> Reloaded {
    decode_all(text.as_bytes(), TabWidth::default()).compared_with(&crate::rope::Rope::new())
}

fn text_of(text: &str, rows: Rows) -> (Reloaded, Rows, Highlights) {
    (loaded(text), rows, Highlights::new(text.len()))
}

/// An editor of `columns` showing a review's unified text.
fn editor_with_review(columns: usize) -> (Editor, ReviewId, WindowId) {
    let mut editor = Editor::default();
    editor.set_frame_size(FrameSize { rows: 30, columns });
    let (id, buffer) = editor.add_review(ProjectId(0), "*review: uncommitted p*".into());
    editor.show(buffer);
    let (text, rows) = unified();
    let _ = editor.set_review_text(id, loaded(text), rows, Highlights::new(text.len()));
    let window = editor.window().id();
    (editor, id, window)
}

/// Puts the review's panes in, as the host does just before it shows them: a command ends
/// panes no window shows.
fn put_panes(editor: &mut Editor, id: ReviewId) {
    let ((old, old_rows), (new, new_rows)) = panes();
    let _ = editor.set_review_panes(id, text_of(old, old_rows), text_of(new, new_rows));
}

/// Puts the panes in and shows them in `window` the way `way`.
fn switch(editor: &mut Editor, id: ReviewId, window: WindowId, way: Way) -> Result<(), NoPair> {
    put_panes(editor, id);
    editor.show_review_panes(window, way)
}

/// Puts the selected window's primary cursor at the start of `line`, as a command would, and
/// ends the command.
fn go(editor: &mut Editor, line: usize) {
    let offset = editor.current().1.text().line_start(line);
    editor.current_mut().0.selections = SelectionSet::new(offset);
    editor.follow_cursor();
}

/// The line the primary cursor of `window` is on, and the buffer it shows.
fn at(editor: &Editor, window: WindowId) -> (usize, crate::buffer::BufferId) {
    let window = editor.window_by_id(window).unwrap();
    let text = editor.shown(window).text();
    (
        text.offset_to_point(window.selections.primary().cursor).row,
        window.buffer(),
    )
}

#[test]
fn the_panes_are_the_reviews_and_say_what_their_lines_are() {
    let (mut editor, id, _) = editor_with_review(200);
    put_panes(&mut editor, id);
    let (old, new) = editor.review_panes(id).unwrap();
    assert_eq!(editor.review_of(old), Some(id));
    assert_eq!(editor.review_of(new), Some(id));
    assert_eq!(
        editor.buffer(old).unwrap().name(),
        "*review: uncommitted p \u{b7} old*"
    );
    assert!(editor.buffer(new).unwrap().read_only().is_some());
    let kind = |buffer, line| {
        editor
            .review_rows_of(buffer)
            .unwrap()
            .at(line)
            .unwrap()
            .kind
    };
    assert_eq!(kind(old, 3), RowKind::Filler);
    assert_eq!(kind(new, 3), label());
}

#[test]
fn panes_of_unequal_lines_are_refused_whole() {
    let mut editor = Editor::default();
    let (id, _) = editor.add_review(ProjectId(0), "*review: uncommitted p*".into());
    let ((old, old_rows), _) = panes();
    let left = editor.set_review_panes(
        id,
        text_of(old, old_rows),
        text_of("one\n", rows(&[(RowKind::File, 1)])),
    );
    assert_eq!(left.panes.len(), 2);
    assert!(
        left.panes
            .iter()
            .all(|pane| pane.holds() == (true, true, true))
    );
    assert_eq!(editor.review_panes(id), None);
}

#[test]
fn a_new_sides_line_goes_to_the_new_pane_selected_and_back() {
    let (mut editor, id, window) = editor_with_review(200);
    // `+new three`.
    go(&mut editor, 7);
    switch(&mut editor, id, window, Way::Across).unwrap();
    let (old, new) = editor.review_panes(id).unwrap();
    let other = editor.pair_of(window).unwrap();
    assert_eq!(at(&editor, window), (5, old));
    assert_eq!(at(&editor, other), (5, new));
    assert_eq!(editor.window().id(), other, "the cursor's side is selected");
    let mut areas = Vec::new();
    editor.review_areas(&mut areas);
    assert_eq!(
        areas,
        [ReviewArea {
            window,
            review: id,
            columns: 200,
            layout: ReviewLayout::Panes(Way::Across),
        }]
    );
    // On the card's text, then back to the unified text.
    go(&mut editor, 4);
    assert!(editor.show_review_unified(other));
    assert_eq!(editor.pair_of(other), None);
    let unified = editor.review_buffer(id).unwrap();
    assert_eq!(at(&editor, other), (6, unified));
    assert_eq!(editor.window().id(), other);
}

#[test]
fn an_old_sides_line_goes_to_the_old_pane_and_a_header_to_the_new() {
    let (mut editor, id, window) = editor_with_review(200);
    // `-old three`.
    go(&mut editor, 3);
    switch(&mut editor, id, window, Way::Down).unwrap();
    let (old, _) = editor.review_panes(id).unwrap();
    assert_eq!(at(&editor, window), (5, old));
    assert_eq!(editor.window().id(), window);
    let mut areas = Vec::new();
    editor.review_areas(&mut areas);
    assert_eq!(areas[0].layout, ReviewLayout::Panes(Way::Down));
    // Unified, on the hunk's header, then two panes again: the new one.
    assert!(editor.show_review_unified(window));
    go(&mut editor, 1);
    switch(&mut editor, id, window, Way::Across).unwrap();
    let (_, new) = editor.review_panes(id).unwrap();
    assert_eq!(at(&editor, editor.window().id()), (1, new));
}

#[test]
fn a_cursor_on_a_filler_row_moves_on_the_way_it_went() {
    let (mut editor, id, window) = editor_with_review(200);
    go(&mut editor, 2);
    switch(&mut editor, id, window, Way::Across).unwrap();
    let (old, _) = editor.review_panes(id).unwrap();
    assert_eq!(at(&editor, window), (2, old));
    // Down onto the card's fillers: on to `-old three`.
    go(&mut editor, 3);
    assert_eq!(at(&editor, window), (5, old));
    // Up onto them: back to `-old two`.
    go(&mut editor, 4);
    assert_eq!(at(&editor, window), (2, old));
    // Down onto the last row, a filler with nothing after it: back up.
    go(&mut editor, 5);
    go(&mut editor, 6);
    assert_eq!(at(&editor, window), (5, old));
}

#[test]
fn unified_from_a_filler_row_takes_the_other_panes_row() {
    let (mut editor, id, window) = editor_with_review(200);
    go(&mut editor, 2);
    switch(&mut editor, id, window, Way::Across).unwrap();
    // The old pane's last row is a filler beside `+new four`: put there as no command would.
    let offset = editor.current().1.text().line_start(6);
    editor.current_mut().0.selections = SelectionSet::new(offset);
    assert!(editor.show_review_unified(window));
    assert_eq!(at(&editor, window), (8, editor.review_buffer(id).unwrap()));
}

#[test]
fn a_switch_too_small_changes_nothing() {
    let (mut editor, id, window) = editor_with_review(12);
    go(&mut editor, 7);
    assert_eq!(
        switch(&mut editor, id, window, Way::Across),
        Err(NoPair::TooSmall)
    );
    assert_eq!(at(&editor, window), (7, editor.review_buffer(id).unwrap()));
    let mut areas = Vec::new();
    editor.review_areas(&mut areas);
    assert_eq!(areas[0].layout, ReviewLayout::Unified);
    assert_eq!(areas[0].columns, 12);
}

#[test]
fn panes_no_window_shows_go_after_a_command_and_with_the_review() {
    let (mut editor, id, window) = editor_with_review(200);
    switch(&mut editor, id, window, Way::Across).unwrap();
    let (old, new) = editor.review_panes(id).unwrap();
    assert!(editor.show_review_unified(window));
    assert!(editor.review_panes(id).is_some(), "until a command ends");
    editor.follow_cursor();
    assert_eq!(editor.review_panes(id), None);
    assert!(editor.buffer(old).is_none() && editor.buffer(new).is_none());
    // Made again, then gone with the review.
    switch(&mut editor, id, window, Way::Across).unwrap();
    let (old, new) = editor.review_panes(id).unwrap();
    editor.remove_review(id);
    assert!(editor.buffer(old).is_none() && editor.buffer(new).is_none());
}

#[test]
fn no_place_crosses_a_switch_from_or_into_a_buffer_that_may_hold_secrets() {
    let (mut editor, id, window) = editor_with_review(200);
    // `+new three`, then the review turned secret, as a yank of a secret kill into isearch does.
    go(&mut editor, 7);
    let unified = editor.review_buffer(id).unwrap();
    editor.buffer_mut(unified).unwrap().treat_as_secret();
    switch(&mut editor, id, window, Way::Across).unwrap();
    let other = editor.pair_of(window).unwrap();
    assert_eq!(at(&editor, window).0, 0);
    assert_eq!(at(&editor, other).0, 0);
    // Back, from a pane, while the unified buffer may hold secrets.
    go(&mut editor, 5);
    assert!(editor.show_review_unified(window));
    assert_eq!(at(&editor, window), (0, unified));
}

#[test]
fn a_long_run_of_filler_rows_is_passed_at_once() {
    let fillers = 1_000_000;
    let long = rows(&[
        (RowKind::File, 1),
        (RowKind::Filler, fillers),
        (RowKind::Hunk { hunk: 1 }, 1),
    ]);
    assert_eq!(super::off_filler(&long, 1, false), Some(fillers + 1));
    assert_eq!(super::off_filler(&long, fillers, true), Some(0));
    assert_eq!(super::off_filler(&long, fillers / 2, true), Some(0));
    let last = rows(&[(RowKind::File, 1), (RowKind::Filler, 3)]);
    assert_eq!(
        super::off_filler(&last, 2, false),
        Some(0),
        "back when nothing is after"
    );
}

#[test]
fn panes_made_again_take_their_text_whole_though_it_did_not_change() {
    let (mut editor, id, window) = editor_with_review(200);
    switch(&mut editor, id, window, Way::Across).unwrap();
    assert!(editor.show_review_unified(window));
    editor.follow_cursor();
    assert_eq!(editor.review_panes(id), None);
    // The view compares its next text with its last one: the same, so no change in it.
    let ((old, old_rows), (new, new_rows)) = panes();
    let same = |text: &str| {
        decode_all(text.as_bytes(), TabWidth::default()).compared_with(&loaded_rope(text))
    };
    let _ = editor.set_review_panes(
        id,
        (same(old), old_rows, Highlights::new(old.len())),
        (same(new), new_rows, Highlights::new(new.len())),
    );
    let (old_buffer, new_buffer) = editor.review_panes(id).unwrap();
    assert_eq!(editor.buffer(old_buffer).unwrap().text().to_string(), old);
    assert_eq!(editor.buffer(new_buffer).unwrap().text().to_string(), new);
}

fn loaded_rope(text: &str) -> crate::rope::Rope {
    crate::rope::Rope::from(text)
}
