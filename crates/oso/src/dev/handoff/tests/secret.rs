//! The tests of what the state handoff does around a buffer that may hold secrets.
//!
//! Must not depend on: a window or the file system; a state is a plain value.

use super::super::{Restore, SECRET_SHOWN, State, check, exit_allowed, export, generation, parse};
use super::{TYPED, absolute, type_text, visiting};
use crate::dev::protocol::EXIT_REFUSED;
use oso_core::buffer::Buffer;
use oso_core::display_map::TabWidth;
use oso_core::editor::Editor;
use oso_core::key::KeySequence;
use oso_core::load::decode_all;
use oso_core::selection::SelectionSet;
use oso_core::window::Scroll;
use std::path::PathBuf;

#[test]
fn a_state_never_holds_text_secret_or_not() {
    for path in ["/p/.env", "/p/notes.txt"] {
        let mut editor = visiting(path, "SECRET-VALUE\n");
        if editor.current().1.is_secret() {
            // Off the screen, as nothing hands over while it is shown.
            editor.toggle_messages();
        }
        let state = export(&editor, &[], None, None).unwrap();
        let line = state.to_string();
        assert!(!line.contains("SECRET-VALUE"), "{line}");
        assert!(line.contains(path), "{line}");
    }
}

#[test]
fn a_secret_file_behind_messages_comes_back_unshown_with_no_position() {
    let path = absolute("/p/.env");
    let mut editor = visiting(&path, &"line\n".repeat(50));
    let selections = SelectionSet::new(25);
    editor.restore_positions(selections, Scroll { top: 2, left: 1 });
    assert_ne!(editor.window().selections.primary().cursor, 0);
    editor.toggle_messages();
    let exported = export(&editor, &[], None, None).unwrap();
    let state = parse(&exported).unwrap();
    assert_eq!(state.buffers[state.shown], None);
    assert!(state.buffers.contains(&Some(PathBuf::from(&path))));
    assert_eq!(state.selections, SelectionSet::default());
    assert_eq!(state.scroll, Scroll::default());
}

/// A new instance that took `state`, as `import-state` builds it, with each file judged by its
/// path and loaded.
fn imported(state: &State) -> Editor {
    let mut editor = Editor::default();
    let scratch = editor.window().buffer();
    let mut shown = scratch;
    for (index, path) in state.buffers.iter().enumerate() {
        let id = path.as_ref().map_or(scratch, |path| {
            let mut buffer = Buffer::visiting(path.clone());
            buffer.apply_load(decode_all(b"KEY=1\n", TabWidth::default()));
            buffer.judge_secret(Some(std::slice::from_ref(path)));
            editor.add_buffer(buffer)
        });
        if index == state.shown {
            shown = id;
        }
    }
    editor.show(shown);
    editor
}

#[test]
fn restarts_go_on_after_one_with_a_secret_file_behind_messages() {
    let mut editor = visiting(&absolute("/p/.env"), "KEY=1\n");
    editor.toggle_messages();
    for restart in 1..=2 {
        let state = parse(&export(&editor, &[], None, None).unwrap()).unwrap();
        editor = imported(&state);
        assert!(!editor.shows_secret(), "restart {restart}");
        assert!(check(&editor).is_ok(), "restart {restart}");
        assert!(editor.buffers().any(|(_, buffer)| buffer.is_secret()));
    }
}

#[test]
fn a_file_not_judged_yet_holds_no_restart_for_being_shown() {
    let mut editor = Editor::default();
    let id = editor.add_buffer(Buffer::visiting(PathBuf::from(absolute("/p/a.txt"))));
    editor.show(id);
    assert!(editor.shows_secret(), "it counts as secret until judged");
    for message in [export(&editor, &[], None, None).err(), check(&editor).err()] {
        assert_ne!(message.as_deref(), Some(SECRET_SHOWN));
    }
}

#[test]
fn positions_waiting_for_another_buffer_do_not_stand_in_for_a_secret_one() {
    let mut editor = visiting(&absolute("/p/.env"), &"line\n".repeat(50));
    editor.restore_positions(SelectionSet::new(25), Scroll { top: 2, left: 1 });
    let other = editor.add_buffer(Buffer::visiting(PathBuf::from(absolute("/p/a.txt"))));
    let waiting = Restore {
        buffer: other,
        selections: SelectionSet::new(600),
        scroll: Scroll { top: 40, left: 0 },
    };
    editor.toggle_messages();
    let state = parse(&export(&editor, &[], None, Some(&waiting)).unwrap()).unwrap();
    assert_eq!(state.selections, SelectionSet::default());
    assert_eq!(state.scroll, Scroll::default());
}

#[test]
fn while_a_secret_file_is_shown_nothing_hands_over_for_one_reason() {
    let mut editor = visiting(&absolute("/p/.env"), "SECRET-VALUE\n");
    assert!(editor.current().1.is_secret());
    let refusals = |editor: &Editor| {
        [
            export(editor, &[], None, None).unwrap_err(),
            check(editor).unwrap_err(),
            exit_allowed(editor, Some(generation(editor))).unwrap_err(),
        ]
    };
    let unmodified = refusals(&editor);
    type_text(&mut editor);
    assert_eq!(refusals(&editor), unmodified, "the same whatever it holds");
    for message in &unmodified[..2] {
        assert_eq!(message, SECRET_SHOWN);
    }
    assert!(unmodified[2].starts_with(EXIT_REFUSED), "{}", unmodified[2]);
    assert!(
        exit_allowed(&editor, None).is_ok(),
        "a person may still say so"
    );
}

#[test]
fn keys_in_progress_typed_where_a_secret_file_was_shown_are_not_handed_over() {
    let mut editor = visiting(&absolute("/p/.env"), "SECRET-VALUE\n");
    let pending: KeySequence = "C-x".parse().unwrap();
    editor.set_keys_in_progress("C-x-");
    editor.toggle_messages();
    let state = parse(&export(&editor, &pending.0, None, None).unwrap()).unwrap();
    assert!(state.pending_keys.is_empty());
    // Typed once it is off the screen, they are.
    editor.set_keys_in_progress("");
    editor.set_keys_in_progress("C-x-");
    let state = parse(&export(&editor, &pending.0, None, None).unwrap()).unwrap();
    assert_eq!(state.pending_keys, pending.0);
}

#[test]
fn a_modified_secret_file_off_the_screen_is_refused_the_same_way() {
    let mut editor = visiting(&absolute("/p/.env"), "SECRET-VALUE\n");
    assert!(editor.current().1.is_secret());
    type_text(&mut editor);
    editor.toggle_messages();
    for message in [
        export(&editor, &[], None, None).unwrap_err(),
        check(&editor).unwrap_err(),
    ] {
        assert!(message.contains(".env"), "{message}");
        assert!(!message.contains("SECRET-VALUE"), "{message}");
        assert!(!message.contains(TYPED), "{message}");
    }
}

#[test]
fn a_saved_file_a_secret_kill_was_yanked_into_is_refused() {
    let mut editor = visiting(&absolute("/p/notes.txt"), "on disk\n");
    let fingerprint = oso_core::kill::Fingerprint { len: 3, hash: 1 };
    editor.take_from_clipboard(oso_core::rope::Rope::from("KEY"), true, fingerprint);
    editor.yank(0, false).unwrap();
    let id = editor.window().buffer();
    let saved = editor.current().1.content_id();
    editor.buffer_mut(id).unwrap().mark_saved(saved);
    assert!(editor.current().1.is_secret() && !editor.current().1.is_modified());
    // Off the screen, as nothing hands over while it is shown.
    editor.toggle_messages();
    for message in [
        export(&editor, &[], None, None).unwrap_err(),
        check(&editor).unwrap_err(),
    ] {
        assert!(message.contains("notes.txt"), "{message}");
        assert!(!message.contains("KEY"), "{message}");
    }
}
