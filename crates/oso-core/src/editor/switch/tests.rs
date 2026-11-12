//! Tests of going between buffers, killing them and their names.
//!
//! Must not depend on: the host's window, threads or I/O.

use std::path::PathBuf;

use super::super::edit_tests::plain_edit;
use super::super::test_buffers::secret_buffer;
use super::*;
use crate::editor::CompletionStyle;
use crate::selection::{Selection, SelectionSet};
use crate::window::Scroll;

/// An editor on `*scratch*` with three ordinary buffers, `a`, `b` and `c`, none shown yet.
fn three() -> (Editor, [BufferId; 3]) {
    let mut editor = Editor::default();
    let ids = ["a", "b", "c"]
        .map(|name| editor.add_buffer(Buffer::from_text(name, "one\ntwo\nthree\nfour\n")));
    (editor, ids)
}

fn shown(editor: &Editor) -> &str {
    editor.current().1.name()
}

fn cursor(editor: &Editor) -> usize {
    editor.window().selections.primary().cursor
}

#[test]
fn coming_back_to_a_buffer_puts_every_cursor_and_the_view_back() {
    let (mut editor, [a, b, _]) = three();
    editor.show(a);
    let at = |cursor| Selection {
        cursor,
        mark: None,
        goal: None,
    };
    let selections = SelectionSet::from_parts(vec![at(4), at(8)], 0).unwrap();
    editor.restore_positions(selections, Scroll { start: 4, left: 0 });
    editor.show(b);
    assert_eq!(
        cursor(&editor),
        0,
        "a buffer new to the window starts at its start"
    );
    editor.show(a);
    let cursors: Vec<_> = editor
        .window()
        .selections
        .iter()
        .map(|s| s.cursor)
        .collect();
    assert_eq!(cursors, [4, 8]);
    assert_eq!(editor.top_row(), 1);
}

#[test]
fn a_place_is_clipped_to_the_text_as_it_is_when_the_window_comes_back() {
    let (mut editor, [a, b, _]) = three();
    editor.show(a);
    editor.restore_positions(SelectionSet::new(18), Scroll::default());
    editor.show(b);
    plain_edit(&mut editor, a, 0..18, "x");
    editor.show(a);
    assert_eq!(cursor(&editor), 2, "the end of the text as it is now");
}

#[test]
fn walking_goes_back_through_every_buffer_and_forward_again() {
    let (mut editor, [a, b, c]) = three();
    editor.show(a);
    editor.show(b);
    editor.show(c);
    // The window showed c, b, a, `*scratch*`, the most recent first.
    let mut seen = Vec::new();
    for _ in 0..4 {
        editor.walk_buffers(1).unwrap();
        seen.push(shown(&editor).to_owned());
    }
    assert_eq!(seen, ["b", "a", "*scratch*", "c"]);
    editor.walk_buffers(1).unwrap();
    editor.walk_buffers(-1).unwrap();
    assert_eq!(shown(&editor), "c", "M-/ after M-, comes back");
    editor.walk_buffers(2).unwrap();
    assert_eq!(shown(&editor), "a", "a count goes that many");
}

#[test]
fn anything_else_that_shows_a_buffer_starts_the_walk_over() {
    let (mut editor, [a, b, c]) = three();
    editor.show(a);
    editor.show(b);
    editor.walk_buffers(1).unwrap();
    assert_eq!(shown(&editor), "a");
    editor.show(c);
    editor.walk_buffers(1).unwrap();
    assert_eq!(
        shown(&editor),
        "a",
        "the most recent other, not the walk's next"
    );
}

#[test]
fn walking_skips_internal_buffers_and_needs_another() {
    let mut editor = Editor::default();
    assert_eq!(editor.walk_buffers(1), Err("No other buffer"));
    editor.toggle_messages();
    editor.walk_buffers(1).unwrap();
    assert_eq!(shown(&editor), SCRATCH, "from `*Messages*` to the user's");
    assert_eq!(editor.walk_buffers(1), Err("No other buffer"));
}

#[test]
fn a_killed_buffer_leaves_the_one_shown_before_and_its_place() {
    let (mut editor, [a, b, _]) = three();
    editor.show(a);
    editor.restore_positions(SelectionSet::new(4), Scroll::default());
    editor.show(b);
    editor.kill_buffer(b);
    assert_eq!(shown(&editor), "a");
    assert_eq!(cursor(&editor), 4);
    assert!(editor.window().places().all(|(id, _)| id != b));
    assert_eq!(editor.take_freed().len(), 1, "freed by the host");
    assert!(editor.take_freed().is_empty());
}

#[test]
fn killing_the_last_buffer_shows_scratch_made_again() {
    let mut editor = Editor::default();
    let scratch = editor.window().buffer();
    editor.kill_buffer(scratch);
    assert_eq!(shown(&editor), SCRATCH);
    assert_ne!(editor.window().buffer(), scratch);
}

#[test]
fn a_person_kills_a_modified_buffer_only_once_asked() {
    let (mut editor, [a, b, _]) = three();
    editor.show(a);
    plain_edit(&mut editor, a, 0..0, "x");
    assert_eq!(
        editor.kill_by_person(a, true),
        Err(KillRefused::Modified),
        "not asked yet"
    );
    assert_eq!(editor.kill_asked(), Some(a));
    assert_eq!(
        editor.kill_by_person(b, true),
        Ok(()),
        "unmodified goes at once"
    );
    assert_eq!(
        editor.kill_asked(),
        None,
        "asked of a, then b went: the answer is spent"
    );
    assert_eq!(editor.kill_by_person(a, false), Err(KillRefused::Modified));
    assert_eq!(editor.kill_by_person(a, true), Ok(()));
    assert!(editor.buffer(a).is_none());
}

#[test]
fn a_person_cannot_kill_an_internal_buffer_or_one_that_saves() {
    let (mut editor, [a, _, _]) = three();
    let messages = editor.messages_buffer();
    assert_eq!(
        editor.kill_by_person(messages, true),
        Err(KillRefused::Internal)
    );
    editor.buffer_mut(a).unwrap().set_saving(true);
    assert_eq!(editor.kill_by_person(a, true), Err(KillRefused::Saving));
    assert_eq!(
        editor.kill_by_person(BufferId(u64::MAX), true),
        Err(KillRefused::Gone)
    );
}

#[test]
fn agent_is_listed_once_made_and_kept_from_a_kill() {
    let (mut editor, _) = three();
    let listed = |editor: &Editor| {
        editor
            .buffers_by_recency()
            .into_iter()
            .filter_map(|id| Some(editor.buffer(id)?.name().to_owned()))
            .collect::<Vec<_>>()
    };
    assert!(
        !listed(&editor).contains(&"*Agent*".to_owned()),
        "not made yet"
    );
    editor.agent_log("run forward-char 1 ok");
    assert!(listed(&editor).contains(&"*Agent*".to_owned()));
    let agent = editor.buffer_named("*Agent*").unwrap();
    // `C-x b *Agent*` goes to it, and makes no buffer of that name.
    let before = editor.buffers().count();
    editor.switch_to_named("*Agent*").unwrap();
    assert_eq!(editor.window().buffer(), agent);
    assert_eq!(editor.buffers().count(), before);
    assert_eq!(
        editor.kill_by_person(agent, true),
        Err(KillRefused::Internal)
    );
    assert!(editor.buffer(agent).is_some());
}

#[test]
fn files_with_one_name_are_told_apart_until_one_goes() {
    let mut editor = Editor::default();
    let work = editor.add_buffer(Buffer::visiting(PathBuf::from("/home/me/work/notes.md")));
    assert_eq!(editor.buffer(work).unwrap().name(), "notes.md");
    let home = editor.add_buffer(Buffer::visiting(PathBuf::from("/home/me/notes.md")));
    assert_eq!(editor.buffer(work).unwrap().name(), "notes.md<work>");
    assert_eq!(editor.buffer(home).unwrap().name(), "notes.md<me>");
    editor.kill_buffer(work);
    assert_eq!(editor.buffer(home).unwrap().name(), "notes.md");
}

#[test]
fn a_secret_buffer_killed_off_the_screen_marks_what_was_around_it() {
    let mut editor = Editor::default();
    let secret = editor.add_buffer(secret_buffer());
    editor.show(secret);
    assert!(editor.shows_secret());
    editor.kill_buffer(secret);
    assert!(!editor.shows_secret());
    assert_eq!(shown(&editor), SCRATCH);
}

#[test]
fn walking_off_an_internal_buffer_empties_the_way_back() {
    let (mut editor, [a, b, _]) = three();
    editor.show(b);
    editor.show(a);
    editor.toggle_messages();
    editor.walk_buffers(-1).unwrap();
    let on = editor.window().buffer();
    assert!(!editor.current().1.is_internal());
    assert_ne!(on, a, "not where the way back began");
    editor.toggle_copies();
    assert_eq!(
        editor
            .place_behind_internal_of(editor.window().id())
            .map(|(id, _, _)| id),
        Some(on)
    );
    editor.leave_all_internal();
    assert_eq!(
        editor.window().buffer(),
        on,
        "where the walk went, not before it"
    );
}

#[test]
fn a_killed_buffer_never_leaves_the_window_on_an_internal_one() {
    let (mut editor, [a, _, _]) = three();
    editor.show(a);
    editor.toggle_messages();
    editor.toggle_messages();
    assert_eq!(editor.window().buffer(), a);
    editor.kill_buffer(a);
    assert!(!editor.current().1.is_internal(), "{}", shown(&editor));
}

#[test]
fn showing_the_buffer_shown_changes_nothing_there() {
    let (mut editor, [a, _, _]) = three();
    editor.show(a);
    editor.restore_positions(SelectionSet::new(4), Scroll { start: 4, left: 0 });
    editor.show(a);
    assert_eq!(cursor(&editor), 4);
    assert_eq!(editor.top_row(), 1);
}

#[test]
fn a_buffer_with_no_file_named_as_a_file_puts_that_file_apart() {
    let mut editor = Editor::default();
    let file = editor.add_buffer(Buffer::visiting(PathBuf::from("/tmp/notes")));
    assert_eq!(editor.buffer(file).unwrap().name(), "notes");
    editor.add_buffer(Buffer::new("notes"));
    assert_eq!(editor.buffer(file).unwrap().name(), "notes<tmp>");
}

#[test]
fn a_preview_shows_the_selected_buffer_where_the_window_was_and_never_a_secret() {
    use crate::complete::Candidate;
    let (mut editor, [a, _, _]) = three();
    editor.show(a);
    editor.restore_positions(SelectionSet::new(8), Scroll { start: 4, left: 0 });
    let secret = editor.add_buffer(secret_buffer());
    editor.show(secret);
    let scratch = editor.buffer_named(SCRATCH).unwrap();
    editor.show(scratch);
    let names = ["a", ".env"].map(|name| Candidate::new(name, "", ""));
    let style = CompletionStyle {
        given_order: true,
        takes_input: true,
        preview: true,
    };
    editor
        .open_completion_as("Switch to buffer:", "buffer", names.into(), style)
        .unwrap();
    assert_eq!(
        editor.preview(),
        Some(Preview::Buffer {
            id: a,
            start: 4,
            cursor: 8
        })
    );
    editor.move_completion(1).unwrap();
    assert_eq!(editor.preview(), Some(Preview::Withheld));
    assert!(!editor.shows_secret(), "nothing of it reached the screen");
}

#[test]
fn a_new_buffers_name_is_bounded_and_holds_no_control_character() {
    let mut editor = Editor::default();
    assert_eq!(
        editor.switch_to_named(&"x".repeat(256)),
        Err("That name is too long")
    );
    assert_eq!(
        editor.switch_to_named("a\nb"),
        Err("A buffer's name holds no control characters")
    );
    assert_eq!(editor.switch_to_named("fine"), Ok(()));
    assert_eq!(shown(&editor), "fine");
}
