//! The tests of the open reviews: adding and closing one, putting in its text with the cursors
//! carried and waiting under a prompt, the backgrounds of the rows in view, and the menu's
//! counts by opening.
//!
//! Must not depend on: `command` or `keymap`, or the host's threads or files.

use std::sync::Arc;

use super::{Count, Editor, ReviewCells, ReviewRequest, ScopeKind};
use crate::buffer::Mode;
use crate::display_map::TabWidth;
use crate::frame::FrameSize;
use crate::load::decode_all;
use crate::project::ProjectId;
use crate::review::{RowKind, Rows, Section, Side};
use crate::selection::SelectionSet;
use crate::syntax::Highlights;
use crate::window::Screen;

/// A review text of `files`, each a path and its added lines, as its header, a hunk's header
/// and the lines, with its rows.
fn review_text(files: &[(&str, usize)]) -> (String, Rows) {
    let mut text = String::new();
    let mut sections = Vec::new();
    for &(path, added) in files {
        text.push_str(&format!("modified  {path}\n@@ -1,0 +1,{added} @@\n"));
        for line in 0..added {
            text.push_str(&format!("+line {line}\n"));
        }
        let mut section = Section::new(path.as_bytes().into());
        section.push(RowKind::File, 1);
        section.push(RowKind::Hunk { hunk: 0 }, 1);
        section.push(
            RowKind::Lines {
                hunk: 0,
                side: Side::New,
                first: 1,
            },
            added,
        );
        sections.push(Arc::new(section));
    }
    (text, Rows::new(sections))
}

fn put_in(editor: &mut Editor, id: super::ReviewId, files: &[(&str, usize)]) {
    let buffer = editor.review_buffer(id).unwrap();
    let (text, rows) = review_text(files);
    let old = editor.buffer(buffer).unwrap().text().clone();
    let reloaded = decode_all(text.as_bytes(), TabWidth::default()).compared_with(&old);
    editor.set_review_text(id, reloaded, rows, Highlights::new(text.len()));
}

fn editor_with_review() -> (Editor, super::ReviewId) {
    let mut editor = Editor::default();
    editor.set_frame_size(FrameSize {
        rows: 12,
        columns: 40,
    });
    let (id, buffer) = editor.add_review(ProjectId(0), "*review: uncommitted p*".into());
    editor.show(buffer);
    (editor, id)
}

#[test]
fn a_review_has_a_read_only_buffer_of_its_own_until_it_closes() {
    let (mut editor, id) = editor_with_review();
    let buffer = editor.review_buffer(id).unwrap();
    let shown = editor.buffer(buffer).unwrap();
    assert!(shown.is_internal());
    assert!(shown.read_only().is_some());
    assert_eq!(shown.mode(), Mode::Review);
    assert!(!shown.wraps(), "a review truncates its lines (Q10)");
    assert_eq!(editor.review_of(buffer), Some(id));
    assert_eq!(editor.reviews().collect::<Vec<_>>(), [(id, ProjectId(0))]);
    assert!(editor.buffers().all(|(other, _)| other != buffer));
    let (second, _) = editor.add_review(ProjectId(0), "*review: staged p*".into());
    assert_ne!(second, id);

    editor.remove_review(id);
    assert!(editor.buffer(buffer).is_none());
    assert_ne!(editor.window().buffer(), buffer);
    assert!(editor.review(id).is_none());
    assert_eq!(editor.take_review_requests(), [ReviewRequest::Close(id)]);
    assert!(editor.take_review_requests().is_empty());
}

#[test]
fn a_new_text_carries_the_cursor_through_what_changed() {
    let (mut editor, id) = editor_with_review();
    put_in(&mut editor, id, &[("a.rs", 3), ("b.rs", 3)]);
    let buffer = editor.review_buffer(id).unwrap();
    let text = editor.buffer(buffer).unwrap().text().clone();
    // On b.rs's second added line.
    let cursor = text.line_start(8);
    editor.current_mut().0.selections = SelectionSet::new(cursor);
    assert_eq!(editor.review_rows(id).unwrap().at(8).unwrap().path, b"b.rs");

    // a.rs gains two lines above it.
    put_in(&mut editor, id, &[("a.rs", 5), ("b.rs", 3)]);
    let text = editor.buffer(buffer).unwrap().text().clone();
    let moved = editor.window().selections.primary().cursor;
    assert_eq!(moved, text.line_start(10));
    let row = editor.review_rows(id).unwrap().at(10).unwrap();
    assert_eq!((row.path, row.line()), (&b"b.rs"[..], Some((Side::New, 2))));
    assert!(
        !editor.buffer(buffer).unwrap().is_modified(),
        "no edit of the person's"
    );
}

#[test]
fn a_new_text_waits_while_a_prompt_is_open() {
    let (mut editor, _) = editor_with_review();
    assert!(!editor.review_waits());
    let kind = crate::minibuffer::Kind::Number { history: None };
    editor.open_prompt("Go to line: ", kind).unwrap();
    assert!(editor.review_waits());
}

#[test]
fn only_the_rows_in_view_take_backgrounds() {
    let (mut editor, id) = editor_with_review();
    put_in(&mut editor, id, &[("a.rs", 40)]);
    let window = editor.window().id();
    let mut screen = Screen::default();
    editor.screen_of(window, &mut screen);
    let mut cells = ReviewCells::default();
    editor.review_cells(window, &screen, &mut cells);
    let text = editor.tiled_regions().text;
    assert_eq!(
        cells.headers.len(),
        1,
        "the file's and the hunk's headers join"
    );
    assert_eq!(cells.headers[0].rows, 2);
    assert_eq!(cells.added.len(), 1);
    assert_eq!(cells.added[0].rows, text.rows - 2);
    assert!(cells.removed.is_empty());

    // Another buffer shows none.
    let scratch = editor.add_buffer(crate::buffer::Buffer::new("other"));
    editor.show(scratch);
    editor.screen_of(window, &mut screen);
    editor.review_cells(window, &screen, &mut cells);
    assert_eq!(cells, ReviewCells::default());
}

#[test]
fn the_menu_keeps_only_its_latest_openings_counts() {
    let mut editor = Editor::default();
    let project = ProjectId(1);
    assert!(editor.scope_menu(project).is_none());
    let first = editor.start_scope_counts(project, &[ScopeKind::Uncommitted, ScopeKind::Staged]);
    let menu = editor.scope_menu(project).unwrap();
    assert_eq!(menu.count(ScopeKind::Uncommitted), Some(Count::Running));
    assert_eq!(menu.count(ScopeKind::Session), None);
    assert!(editor.set_scope_count(project, first, ScopeKind::Staged, Count::Files(3)));

    let second = editor.start_scope_counts(project, &[ScopeKind::Staged]);
    assert_ne!(first, second);
    assert!(!editor.set_scope_count(project, first, ScopeKind::Staged, Count::Files(9)));
    editor.set_branch_base(project, first, Some("origin/main".into()));
    let menu = editor.scope_menu(project).unwrap();
    assert_eq!(menu.count(ScopeKind::Staged), Some(Count::Running));
    assert_eq!(menu.count(ScopeKind::Uncommitted), None);
    assert_eq!(menu.base(), None);
    assert_eq!(menu.generation(), second);

    editor.set_branch_base(project, second, Some("origin/main".into()));
    editor.set_last_scope(project, ScopeKind::Branch);
    editor.set_baseline_taken(project, Some(60));
    let menu = editor.scope_menu(project).unwrap();
    assert_eq!(menu.base(), Some("origin/main"));
    assert_eq!(menu.last(), Some(ScopeKind::Branch));
    assert_eq!(menu.baseline_taken(), Some(60));
}
