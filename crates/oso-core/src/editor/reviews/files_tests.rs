//! A review's files pane on the editor (ADR 0122, step 6b): shown at the left of the review's
//! area, a pair's too, its text put in with each row kept by its file, its cursor following the
//! review's file, and hidden, closed or dropped as nothing shows it or its review; and the
//! filter kept beside the review.
//!
//! Must not depend on: `command` or `keymap`, or the host's threads or files.

use std::sync::Arc;

use crate::display_map::TabWidth;
use crate::editor::{Editor, NoPair, ReviewId};
use crate::frame::{FrameSize, Way};
use crate::load::{Reloaded, decode_all};
use crate::project::ProjectId;
use crate::review::{Filter, RowKind, Rows, Section, Side};
use crate::selection::SelectionSet;
use crate::syntax::Highlights;
use crate::window::WindowId;

fn loaded(text: &str) -> Reloaded {
    decode_all(text.as_bytes(), TabWidth::default()).compared_with(&crate::rope::Rope::new())
}

/// Rows of one section a file, each `lines` long, its first its header.
fn rows(files: &[(&str, usize)]) -> Rows {
    let sections = files
        .iter()
        .map(|(path, lines)| {
            let mut section = Section::new(path.as_bytes().into());
            section.push(RowKind::File, 1);
            if *lines > 1 {
                section.push(
                    RowKind::Lines {
                        hunk: 0,
                        side: Side::New,
                        first: 1,
                    },
                    lines - 1,
                );
            }
            Arc::new(section)
        })
        .collect();
    Rows::new(sections)
}

/// The text of `files`' rows: each its path, then its lines.
fn text(files: &[(&str, usize)]) -> String {
    let mut out = String::new();
    for (path, lines) in files {
        out.push_str(path);
        out.push('\n');
        for line in 1..*lines {
            out.push_str(&format!("+{line}\n"));
        }
    }
    out
}

const REVIEW: [(&str, usize); 3] = [("a.rs", 3), ("b.rs", 4), ("c.rs", 2)];
const PANE: [(&str, usize); 3] = [("a.rs", 1), ("b.rs", 1), ("c.rs", 1)];

/// An editor of `columns` showing a review's unified text of [`REVIEW`].
fn editor_with_review(columns: usize) -> (Editor, ReviewId, WindowId) {
    let mut editor = Editor::default();
    editor.set_frame_size(FrameSize { rows: 30, columns });
    let (id, buffer) = editor.add_review(ProjectId(0), "*review: uncommitted p*".into());
    editor.show(buffer);
    let review = text(&REVIEW);
    let _ = editor.set_review_text(
        id,
        loaded(&review),
        rows(&REVIEW),
        Highlights::new(review.len()),
        &|_, _, number| Some(number),
    );
    let window = editor.window().id();
    (editor, id, window)
}

/// Puts the pane's text of `files` in.
fn put_pane(editor: &mut Editor, id: ReviewId, files: &[(&str, usize)]) {
    let pane = text(files);
    let _ = editor.set_review_files(id, loaded(&pane), rows(files), Highlights::new(pane.len()));
}

/// Puts the selected window's primary cursor at the start of `line` and ends the command.
fn go(editor: &mut Editor, line: usize) {
    let offset = editor.current().1.text().line_start(line);
    editor.current_mut().0.selections = SelectionSet::new(offset);
    editor.follow_cursor();
}

/// The line `window`'s primary cursor is on.
fn line_of(editor: &Editor, window: WindowId) -> usize {
    let window = editor.window_by_id(window).unwrap();
    let text = editor.shown(window).text();
    text.offset_to_point(window.selections.primary().cursor).row
}

fn columns_of(editor: &Editor, window: WindowId) -> usize {
    editor.frame().regions(window).unwrap().outer().columns
}

#[test]
fn the_pane_goes_at_the_left_of_the_review_and_takes_its_text_whole() {
    let (mut editor, id, window) = editor_with_review(120);
    let pane = editor.show_review_files(id, window).unwrap();
    assert_ne!(pane, window);
    assert_eq!(editor.window().id(), window, "the review stays selected");
    assert_eq!(columns_of(&editor, pane), 40);
    assert_eq!(columns_of(&editor, window), 80);
    let buffer = editor.review_files(id).unwrap();
    let shown = editor.buffer(buffer).unwrap();
    assert_eq!(shown.name(), "*review files: uncommitted p*");
    assert_eq!(shown.mode(), crate::buffer::Mode::ReviewFiles);
    assert!(shown.is_internal() && shown.read_only().is_some());
    assert_eq!(
        editor.review_of(buffer),
        None,
        "the pane is no area of the review"
    );
    put_pane(&mut editor, id, &PANE);
    assert_eq!(
        editor.buffer(buffer).unwrap().text().to_string(),
        text(&PANE)
    );
    // Shown again, the same window.
    assert_eq!(editor.show_review_files(id, window), Ok(pane));
}

#[test]
fn beside_a_pair_it_goes_at_the_left_of_both_panes() {
    let (mut editor, id, window) = editor_with_review(200);
    let side = |editor: &mut Editor| {
        let pane = text(&REVIEW);
        let side = || (loaded(&pane), rows(&REVIEW), Highlights::new(pane.len()));
        let _ = editor.set_review_panes(id, side(), side(), &|_, _, number| Some(number));
    };
    side(&mut editor);
    editor.show_review_panes(window, Way::Across).unwrap();
    let other = editor.pair_of(window).unwrap();
    let pane = editor.show_review_files(id, other).unwrap();
    assert_eq!(columns_of(&editor, pane), 40);
    let order: Vec<WindowId> = editor.frame().windows().iter().map(|(id, _)| *id).collect();
    assert_eq!(order, [pane, window, other]);
    assert_eq!(editor.pair_of(window), Some(other), "still a pair");
}

#[test]
fn a_text_keeps_each_rows_file_and_the_cursor_follows_the_reviews() {
    let (mut editor, id, window) = editor_with_review(120);
    let pane = editor.show_review_files(id, window).unwrap();
    put_pane(&mut editor, id, &PANE);
    // On b.rs's second line in the review: the pane's cursor goes to b.rs.
    go(&mut editor, 4);
    assert_eq!(line_of(&editor, pane), 1);
    go(&mut editor, 7);
    assert_eq!(line_of(&editor, pane), 2, "c.rs");
    // A file put in above: the pane's row stays on c.rs.
    put_pane(
        &mut editor,
        id,
        &[("0.rs", 1), ("a.rs", 1), ("b.rs", 1), ("c.rs", 1)],
    );
    assert_eq!(line_of(&editor, pane), 3);
}

#[test]
fn the_cursor_does_not_follow_from_the_pane_nor_while_the_review_may_hold_secrets() {
    let (mut editor, id, window) = editor_with_review(120);
    let pane = editor.show_review_files(id, window).unwrap();
    put_pane(&mut editor, id, &PANE);
    editor.select_window(pane);
    go(&mut editor, 2);
    assert_eq!(
        line_of(&editor, pane),
        2,
        "the person's own move in the pane"
    );
    editor.select_window(window);
    let review = editor.review_buffer(id).unwrap();
    editor.buffer_mut(review).unwrap().treat_as_secret();
    go(&mut editor, 0);
    assert_eq!(
        line_of(&editor, pane),
        2,
        "no place leaves a buffer that may hold secrets"
    );
}

#[test]
fn hiding_gives_the_columns_back_and_the_pane_goes_with_its_review() {
    let (mut editor, id, window) = editor_with_review(120);
    let pane = editor.show_review_files(id, window).unwrap();
    assert!(editor.hide_review_files(id));
    assert!(editor.window_by_id(pane).is_none());
    assert_eq!(columns_of(&editor, window), 120);
    assert_eq!(editor.review_files(id), None);
    assert!(!editor.hide_review_files(id), "nothing to hide");
    // Once no window shows the review, its pane goes after the command.
    let pane = editor.show_review_files(id, window).unwrap();
    editor.select_window(window);
    let other = editor.add_buffer(crate::buffer::Buffer::from_text("notes", "x"));
    editor.show(other);
    editor.follow_cursor();
    assert!(editor.window_by_id(pane).is_none());
    assert_eq!(editor.review_files(id), None);
}

#[test]
fn a_pane_no_window_shows_is_dropped_and_a_closed_review_takes_it_along() {
    let (mut editor, id, window) = editor_with_review(120);
    let pane = editor.show_review_files(id, window).unwrap();
    editor.select_window(pane);
    editor.delete_window().unwrap();
    editor.follow_cursor();
    assert_eq!(editor.review_files(id), None);
    let pane = editor.show_review_files(id, window).unwrap();
    editor.remove_review(id);
    assert!(editor.window_by_id(pane).is_none());
    assert_eq!(editor.frame().windows().len(), 1);
}

#[test]
fn there_is_no_pane_without_room_or_a_window_of_the_review() {
    let (mut editor, id, window) = editor_with_review(20);
    assert_eq!(editor.show_review_files(id, window), Err(NoPair::TooSmall));
    let (mut editor, id, _) = editor_with_review(120);
    let other = editor.split_window(Way::Across, false).unwrap();
    editor.select_window(other);
    let notes = editor.add_buffer(crate::buffer::Buffer::from_text("notes", "x"));
    editor.show(notes);
    assert_eq!(editor.show_review_files(id, other), Err(NoPair::NoBuffer));
    // A text with no pane is all left over.
    let pane = text(&PANE);
    let left = editor.set_review_files(id, loaded(&pane), rows(&PANE), Highlights::new(pane.len()));
    assert_eq!(left.holds(), (true, true, true));
}

#[test]
fn the_filter_is_kept_beside_the_review() {
    let (mut editor, id, _) = editor_with_review(120);
    assert!(editor.review_filter(id).is_none());
    editor.set_review_filter(id, Filter::new("src"));
    assert_eq!(editor.review_filter(id).map(Filter::text), Some("src"));
    editor.set_review_filter(id, None);
    assert!(editor.review_filter(id).is_none());
}

#[test]
fn a_cursor_on_a_file_the_filter_now_hides_goes_to_a_files_header() {
    let (mut editor, id, window) = editor_with_review(120);
    // On b.rs's second line.
    go(&mut editor, 5);
    let shown = [("a.rs", 3), ("c.rs", 2)];
    let review = text(&shown);
    let _ = editor.set_review_text(
        id,
        loaded(&review),
        rows(&shown),
        Highlights::new(review.len()),
        &|_, _, number| Some(number),
    );
    let line = line_of(&editor, window);
    let rows = editor.review_rows(id).unwrap();
    assert_eq!(
        rows.at(line).unwrap().kind,
        RowKind::File,
        "on a header, at {line}"
    );
}
