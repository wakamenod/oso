//! Tests of the global mark ring: what a push adds, its bound, places that follow edits, undos
//! and reloads and go with their buffer, `pop-global-mark` going round, and what it shows of
//! a buffer that may hold secrets.
//!
//! Must not depend on: the host or its commands; these drive the editor alone.

use std::path::PathBuf;

use super::*;
use crate::buffer::{Buffer, Replacement};

fn cursor(editor: &Editor) -> (BufferId, usize) {
    let window = editor.window();
    (window.buffer(), window.selections.primary().cursor)
}

/// Sets the mark at the primary cursor, after putting it at `at`, as `C-SPC` does, and ends
/// the command.
fn push_at(editor: &mut Editor, at: usize) {
    let selections = &mut editor.current_mut().0.selections;
    selections.move_primary(at);
    selections.push_marks();
    editor.end_command();
}

fn places(editor: &Editor) -> Vec<(BufferId, usize)> {
    editor.global_marks().iter().collect()
}

/// An editor with the buffers `a` and `b`, showing `a`.
fn two() -> (Editor, BufferId, BufferId) {
    let mut editor = Editor::with_buffer(Buffer::from_text("a", "alpha beta gamma\n"));
    let a = editor.window().buffer();
    let b = editor.add_buffer(Buffer::from_text("b", "one two three\n"));
    (editor, a, b)
}

#[test]
fn a_push_adds_a_place_only_in_another_buffer_than_the_newest() {
    let (mut editor, a, b) = two();
    assert!(editor.global_marks().is_empty());
    push_at(&mut editor, 6);
    assert_eq!(places(&editor), [(a, 6)]);
    push_at(&mut editor, 11);
    assert_eq!(places(&editor), [(a, 6)], "the same buffer as the newest");
    editor.show(b);
    editor.end_command();
    assert_eq!(places(&editor), [(a, 6)], "showing a buffer pushes nothing");
    push_at(&mut editor, 4);
    assert_eq!(places(&editor), [(b, 4), (a, 6)]);
    editor.show(a);
    push_at(&mut editor, 0);
    assert_eq!(places(&editor), [(a, 0), (b, 4), (a, 6)]);
}

#[test]
fn a_push_before_the_window_shows_another_buffer_counts() {
    let (mut editor, a, b) = two();
    let selections = &mut editor.current_mut().0.selections;
    selections.move_primary(6);
    selections.push_marks();
    editor.show(b);
    editor.end_command();
    assert_eq!(places(&editor), [(a, 6)]);
}

#[test]
fn the_ring_keeps_the_newest_sixteen() {
    let mut editor = Editor::with_buffer(Buffer::from_text("0", "text\n"));
    let mut ids = vec![editor.window().buffer()];
    for n in 1..MARK_RING_SIZE + 3 {
        ids.push(editor.add_buffer(Buffer::from_text(n.to_string(), "text\n")));
    }
    for id in &ids {
        editor.show(*id);
        push_at(&mut editor, 1);
    }
    assert_eq!(editor.global_marks().len(), MARK_RING_SIZE);
    assert_eq!(
        editor.global_marks().iter().next(),
        Some((ids[ids.len() - 1], 1))
    );
    assert_eq!(editor.global_marks().iter().last(), Some((ids[3], 1)));
}

#[test]
fn places_follow_edits_and_undo_and_a_killed_buffer_s_go() {
    let (mut editor, a, b) = two();
    push_at(&mut editor, 6);
    editor.show(b);
    push_at(&mut editor, 4);
    editor.show(a);
    editor.begin_undo_group();
    let edit = Replacement {
        range: 0..0,
        text: "my ",
    };
    editor.apply_edits(a, &[edit]).unwrap();
    editor.end_undo_group();
    assert_eq!(
        places(&editor),
        [(b, 4), (a, 9)],
        "after the text put before it"
    );
    let at_the_place = Replacement {
        range: 9..9,
        text: "new ",
    };
    editor.apply_edits(a, &[at_the_place]).unwrap();
    assert_eq!(places(&editor), [(b, 4), (a, 9)], "before text put at it");
    editor.undo().unwrap();
    assert_eq!(places(&editor), [(b, 4), (a, 9)]);
    editor.kill_buffer(b);
    assert_eq!(places(&editor), [(a, 9)]);
}

#[test]
fn pop_goes_to_the_newest_with_one_cursor_and_goes_round() {
    let (mut editor, a, b) = two();
    push_at(&mut editor, 6);
    editor.show(b);
    push_at(&mut editor, 4);
    let set = crate::selection::SelectionSet::from_parts(
        vec![
            crate::selection::Selection::at(0),
            crate::selection::Selection::at(8),
        ],
        1,
    )
    .unwrap();
    editor.current_mut().0.selections = set;
    assert_eq!(editor.pop_global_mark(), Ok(()));
    assert_eq!(cursor(&editor), (b, 4), "the newest, in this buffer");
    assert_eq!(editor.window().selections.len(), 1);
    editor.end_command();
    assert_eq!(places(&editor), [(a, 6), (b, 4)], "the newest went last");
    assert_eq!(editor.pop_global_mark(), Ok(()));
    assert_eq!(cursor(&editor), (a, 6));
    editor.end_command();
    assert_eq!(places(&editor), [(b, 4), (a, 6)], "no mark pushed");
    assert_eq!(editor.pop_global_mark(), Ok(()));
    assert_eq!(cursor(&editor), (b, 4), "round again");
}

#[test]
fn pop_with_no_place_says_so_and_a_place_past_the_text_is_clipped() {
    let (mut editor, a, b) = two();
    assert_eq!(editor.pop_global_mark(), Err("No global mark set"));
    push_at(&mut editor, 16);
    editor.show(b);
    let all = Replacement {
        range: 0..17,
        text: "",
    };
    editor.apply_edits(a, &[all]).unwrap();
    assert_eq!(editor.pop_global_mark(), Ok(()));
    assert_eq!(cursor(&editor), (a, 0));
}

#[test]
fn debug_shows_only_how_many_places() {
    let (mut editor, _, _) = two();
    push_at(&mut editor, 6);
    assert_eq!(
        format!("{:?}", editor.global_marks()),
        "GlobalMarks { places: 1 }"
    );
}

/// A buffer visiting `path`, judged, and shown.
fn visiting(editor: &mut Editor, path: &str, text: &str) -> BufferId {
    let mut buffer = Buffer::visiting(PathBuf::from(path));
    buffer.apply_load(crate::load::decode_all(
        text.as_bytes(),
        crate::display_map::TabWidth::default(),
    ));
    buffer.judge_secret(None);
    let id = editor.add_buffer(buffer);
    editor.show(id);
    id
}

#[test]
fn the_agent_goes_to_no_place_in_a_buffer_that_may_hold_secrets() {
    let (mut editor, a, _) = two();
    push_at(&mut editor, 6);
    let secret = visiting(&mut editor, "/p/.env", "KEY=value\n");
    assert!(editor.buffer(secret).unwrap().is_secret());
    push_at(&mut editor, 4);
    editor.show(a);
    let refused = editor.for_agent(Editor::pop_global_mark);
    assert_eq!(refused, Err("That place's buffer is not for the agent"));
    assert_eq!(cursor(&editor).0, a);
    assert_eq!(places(&editor), [(secret, 4), (a, 6)], "the ring as it was");
    // The person's own place is theirs to go to.
    assert_eq!(editor.pop_global_mark(), Ok(()));
    assert_eq!(cursor(&editor), (secret, 4));
}

#[test]
fn the_agent_goes_to_no_place_in_a_buffer_that_turned_secret_after_the_push() {
    let (mut editor, a, b) = two();
    push_at(&mut editor, 6);
    editor.show(b);
    push_at(&mut editor, 4);
    editor.show(a);
    assert!(editor.buffer_mut(b).unwrap().treat_as_secret());
    let refused = editor.for_agent(Editor::pop_global_mark);
    assert_eq!(refused, Err("That place's buffer is not for the agent"));
    assert_eq!(places(&editor), [(b, 4), (a, 6)], "the ring as it was");
    assert_eq!(cursor(&editor).0, a);
}
