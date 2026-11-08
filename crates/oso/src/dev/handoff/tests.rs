//! The tests of the state handoff.
//!
//! Must not depend on: a window or the file system; a state is a plain value.

use super::{
    Frame, MAX_BUFFERS, MAX_FRAME_SIDE, MAX_SELECTIONS, Restore, STATE_VERSION, check,
    exit_allowed, export, generation, import_allowed, parse,
};
use crate::dev::protocol::EXIT_REFUSED;
use oso_core::buffer::{Buffer, FileStamp, Replacement};
use oso_core::display_map::TabWidth;
use oso_core::editor::{Editor, SCRATCH};
use oso_core::key::KeySequence;
use oso_core::load::decode_all;
use oso_core::selection::{Goal, Selection, SelectionSet};
use oso_core::window::{Scroll, ViewSize};
use serde_json::{Value, json};
use std::path::PathBuf;

/// An absolute path on any system.
pub(super) fn absolute(path: &str) -> String {
    if cfg!(windows) {
        format!("C:{path}")
    } else {
        path.to_owned()
    }
}

/// An editor showing `path` with `content`, loaded.
pub(super) fn visiting(path: &str, content: &str) -> Editor {
    let mut editor = Editor::default();
    let mut buffer = Buffer::visiting(PathBuf::from(path));
    buffer.apply_load(decode_all(content.as_bytes(), TabWidth::default()));
    buffer.judge_secret(Some(&[PathBuf::from(path)]));
    let id = editor.add_buffer(buffer);
    editor.show(id);
    editor.set_view_size(ViewSize::new(5, 20));
    editor
}

/// Text the tests type, which no message may quote.
const TYPED: &str = "TYPED-TEXT";

/// Types into the selected window's buffer, as M2-4 will.
fn type_text(editor: &mut Editor) {
    let id = editor.window().buffer();
    let edit = Replacement {
        range: 0..0,
        text: TYPED,
    };
    editor.apply_edits(id, &[edit]).unwrap();
}

/// A change that spoils a state.
type Mutation = Box<dyn Fn(&mut Value)>;

fn with(mutate: impl FnOnce(&mut Value)) -> Value {
    let editor = visiting(&absolute("/p/a.txt"), "abc\n");
    let mut state = export(&editor, &[], None, None).unwrap();
    mutate(&mut state);
    state
}

#[test]
fn a_state_round_trips() {
    let mut editor = visiting(&absolute("/p/a.txt"), &"line\n".repeat(50));
    let selections = vec![
        Selection::at(5),
        Selection {
            cursor: 11,
            mark: None,
            goal: Some(Goal::Column(3)),
        },
        Selection {
            cursor: 15,
            mark: None,
            goal: Some(Goal::LineEnd),
        },
    ];
    editor.restore_positions(
        SelectionSet::from_parts(selections.clone(), 1).unwrap(),
        Scroll { top: 2, left: 1 },
    );
    let pending: KeySequence = "C-x 4".parse().unwrap();
    let frame = Frame {
        position: Some((-10, 20)),
        width: 800,
        height: 600,
    };
    let exported = export(&editor, &pending.0, Some(frame), None).unwrap();
    assert_eq!(exported["version"], STATE_VERSION);

    let state = parse(&exported).unwrap();
    assert_eq!(
        state.buffers,
        [None, Some(PathBuf::from(absolute("/p/a.txt")))]
    );
    assert_eq!(state.shown, 1);
    assert_eq!(
        state.selections.iter().copied().collect::<Vec<_>>(),
        selections
    );
    assert_eq!(state.selections.primary().cursor, 11);
    assert_eq!(state.scroll, Scroll { top: 2, left: 1 });
    assert_eq!(state.pending_keys, pending.0);
    assert_eq!(state.frame, Some(frame));

    // Where the window system does not tell the place, the size still counts.
    let frame = Frame {
        position: None,
        ..frame
    };
    let exported = export(&editor, &[], Some(frame), None).unwrap();
    assert_eq!(parse(&exported).unwrap().frame, Some(frame));
}

#[test]
fn positions_still_waiting_for_their_file_are_the_ones_exported() {
    let mut editor = Editor::default();
    let path = PathBuf::from(absolute("/p/a.txt"));
    let mut buffer = Buffer::visiting(path.clone());
    buffer.judge_secret(Some(&[path]));
    let id = editor.add_buffer(buffer);
    editor.show(id);
    let restore = restore(600, 40, &editor);
    let state = parse(&export(&editor, &[], None, Some(&restore)).unwrap()).unwrap();
    assert_eq!(state.selections.primary().cursor, 600);
    assert_eq!(state.scroll.top, 40);
}

#[test]
fn text_with_no_file_is_refused_not_lost() {
    let editor = Editor::with_buffer(Buffer::from_text("notes", "unsaved"));
    let message = export(&editor, &[], None, None).unwrap_err();
    assert!(message.contains("notes"), "{message}");
    assert!(!message.contains("unsaved"), "{message}");
}

#[test]
fn a_modified_file_is_refused_by_name_without_its_text() {
    let mut editor = visiting(&absolute("/p/notes.txt"), "on disk\n");
    type_text(&mut editor);
    for message in [
        export(&editor, &[], None, None).unwrap_err(),
        check(&editor).unwrap_err(),
    ] {
        assert!(message.contains("notes.txt"), "{message}");
        assert!(message.contains("unsaved"), "{message}");
        assert!(!message.contains(TYPED), "{message}");
        assert!(!message.contains("on disk"), "{message}");
    }
}

#[test]
fn text_in_scratch_is_refused_and_an_empty_scratch_is_not() {
    let mut editor = Editor::default();
    assert!(export(&editor, &[], None, None).is_ok());
    assert!(check(&editor).is_ok());
    type_text(&mut editor);
    assert!(check(&editor).unwrap_err().contains(SCRATCH));
    assert!(export(&editor, &[], None, None).is_err());
    // Emptied again, it holds nothing, and undo history is not carried.
    let id = editor.window().buffer();
    let all = Replacement {
        range: 0..TYPED.len(),
        text: "",
    };
    editor.apply_edits(id, &[all]).unwrap();
    assert!(editor.current().1.is_modified());
    assert!(check(&editor).is_ok());
    assert!(export(&editor, &[], None, None).is_ok());
}

#[test]
fn saving_or_undoing_lets_a_restart_through() {
    let mut editor = visiting(&absolute("/p/a.txt"), "abc\n");
    type_text(&mut editor);
    assert!(check(&editor).is_err());
    editor.undo().unwrap();
    assert!(check(&editor).is_ok());
    editor.redo().unwrap();
    assert!(check(&editor).is_err());
    let (_, buffer) = editor.current();
    let content = buffer.content_id();
    let id = editor.window().buffer();
    editor.buffer_mut(id).unwrap().mark_saved(content);
    assert!(check(&editor).is_ok());
    assert!(export(&editor, &[], None, None).is_ok());
}

#[test]
fn export_and_check_report_the_same_generation() {
    let editor = visiting(&absolute("/p/a.txt"), "abc\n");
    let exported = export(&editor, &[], None, None).unwrap();
    assert_eq!(exported["generation"], generation(&editor));
    assert_eq!(check(&editor).unwrap()["generation"], generation(&editor));
    assert_eq!(exported["version"], 1, "the format stays version 1");
    assert!(parse(&exported).is_ok(), "a reader takes it as before");
}

#[test]
fn any_edit_changes_the_generation_and_undoing_it_restores_it() {
    let mut editor = visiting(&absolute("/p/a.txt"), "abc\n");
    let before = generation(&editor);
    assert_eq!(generation(&editor), before, "it does not move by itself");
    type_text(&mut editor);
    let edited = generation(&editor);
    assert_ne!(edited, before);
    type_text(&mut editor);
    assert_ne!(generation(&editor), edited, "a second edit counts too");
    editor.undo().unwrap();
    assert_eq!(generation(&editor), edited);
    editor.undo().unwrap();
    assert_eq!(generation(&editor), before);
}

#[test]
fn a_new_buffer_or_a_change_to_another_buffer_changes_the_generation() {
    let mut editor = visiting(&absolute("/p/a.txt"), "abc\n");
    let before = generation(&editor);
    let (scratch, _) = editor.buffers().next().unwrap();
    editor.show(scratch);
    assert_eq!(
        generation(&editor),
        before,
        "showing another changes nothing"
    );
    type_text(&mut editor);
    assert_ne!(generation(&editor), before, "not the buffer that was shown");

    let mut editor = visiting(&absolute("/p/a.txt"), "abc\n");
    let before = generation(&editor);
    editor.add_buffer(Buffer::visiting(PathBuf::from(absolute("/p/b.txt"))));
    assert_ne!(generation(&editor), before);
}

#[test]
fn a_file_still_loading_does_not_change_the_generation() {
    let mut editor = Editor::default();
    let id = editor.add_buffer(Buffer::visiting(PathBuf::from("/p/a.txt")));
    let before = generation(&editor);
    let buffer = editor.buffer_mut(id).unwrap();
    buffer.apply_load(decode_all(b"abc\n", TabWidth::default()));
    assert_eq!(generation(&editor), before);
}

#[test]
fn exit_needs_the_generation_if_it_is_given_one() {
    let mut editor = visiting(&absolute("/p/a.txt"), "abc\n");
    let exported = generation(&editor);
    assert_eq!(exit_allowed(&editor, Some(exported)), Ok(()));
    assert_eq!(exit_allowed(&editor, None), Ok(()));
    type_text(&mut editor);
    let message = exit_allowed(&editor, Some(exported)).unwrap_err();
    assert!(message.starts_with(EXIT_REFUSED), "{message}");
    assert!(!message.contains(TYPED), "{message}");
    assert_eq!(
        exit_allowed(&editor, None),
        Ok(()),
        "someone who asks for it without one gets it, unsaved text or not"
    );
    assert_eq!(exit_allowed(&editor, Some(generation(&editor))), Ok(()));
}

#[test]
fn an_import_needs_an_empty_scratch() {
    let mut editor = Editor::default();
    assert_eq!(import_allowed(&editor), Ok(()));
    type_text(&mut editor);
    let message = import_allowed(&editor).unwrap_err();
    assert!(message.contains(SCRATCH), "{message}");
    assert!(!message.contains(TYPED), "{message}");
    // Emptied again, it holds nothing to hide, as for a handover.
    let id = editor.window().buffer();
    let all = Replacement {
        range: 0..TYPED.len(),
        text: "",
    };
    editor.apply_edits(id, &[all]).unwrap();
    assert!(editor.current().1.is_modified());
    assert_eq!(import_allowed(&editor), Ok(()));

    let editor = visiting(&absolute("/p/a.txt"), "abc\n");
    let message = import_allowed(&editor).unwrap_err();
    assert!(message.contains("opened"), "{message}");
}

#[cfg(unix)]
#[test]
fn a_path_that_is_not_utf8_is_refused() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    let mut editor = Editor::default();
    let path = PathBuf::from(OsStr::from_bytes(b"/p/\xff"));
    let mut buffer = Buffer::visiting(path.clone());
    buffer.judge_secret(Some(&[path]));
    let id = editor.add_buffer(buffer);
    editor.show(id);
    assert!(
        export(&editor, &[], None, None)
            .unwrap_err()
            .contains("not UTF-8")
    );
}

#[test]
fn a_state_too_long_for_a_line_is_refused() {
    let long = format!("/{}", "a".repeat(super::MAX_STATE));
    let mut editor = Editor::default();
    editor.add_buffer(Buffer::visiting(PathBuf::from(long)));
    let message = export(&editor, &[], None, None).unwrap_err();
    assert!(message.contains("bytes"), "{message}");
}

#[test]
fn another_version_is_refused() {
    for version in [json!(STATE_VERSION + 1), json!(0), json!("1"), json!(null)] {
        let state = with(|state| state["version"] = version.clone());
        let message = parse(&state).unwrap_err();
        assert!(message.contains("version"), "{message}");
    }
    // A long value is not echoed.
    let state = with(|state| state["version"] = json!("x".repeat(100_000)));
    assert!(parse(&state).unwrap_err().len() < 100);
    assert!(parse(&json!([])).is_err());
}

#[test]
fn bad_fields_are_refused() {
    let bad: [(&str, Mutation); 13] = [
        ("buffers", Box::new(|s| s["buffers"] = json!(1))),
        ("path", Box::new(|s| s["buffers"][1]["path"] = json!(1))),
        (
            "absolute",
            Box::new(|s| s["buffers"][1]["path"] = json!("a.txt")),
        ),
        ("id", Box::new(|s| s["buffers"][1]["id"] = json!("x"))),
        ("one window", Box::new(|s| s["windows"] = json!([]))),
        ("one window", Box::new(|s| s["selected"] = json!(1))),
        (
            "does not have",
            Box::new(|s| s["windows"][0]["buffer"] = json!(99)),
        ),
        (
            "primary",
            Box::new(|s| s["windows"][0]["primary"] = json!(5)),
        ),
        (
            "cursor",
            Box::new(|s| s["windows"][0]["selections"][0]["cursor"] = json!(-1)),
        ),
        (
            "goal",
            Box::new(|s| s["windows"][0]["selections"][0]["goal"] = json!(1)),
        ),
        (
            "invalid key",
            Box::new(|s| s["pending_keys"] = json!("C-nope")),
        ),
        (
            "width",
            Box::new(|s| {
                s["frame"] = json!({ "x": 0, "y": 0, "width": MAX_FRAME_SIDE + 1, "height": 1 });
            }),
        ),
        (
            "width",
            Box::new(|s| {
                s["frame"] = json!({ "x": 0, "y": 0, "width": 0, "height": 1 });
            }),
        ),
    ];
    for (expected, mutate) in bad {
        let state = with(|state| mutate(state));
        let message = parse(&state).unwrap_err();
        assert!(message.contains(expected), "{expected}: {message}");
    }
    let many = vec![json!({ "id": 0 }); MAX_BUFFERS + 1];
    let state = with(|state| state["buffers"] = json!(many));
    assert!(parse(&state).unwrap_err().contains("more than"));
    let many = vec![json!({ "cursor": 0 }); MAX_SELECTIONS + 1];
    let state = with(|state| state["windows"][0]["selections"] = json!(many));
    assert!(parse(&state).unwrap_err().contains("selections"));
    let state = with(|state| {
        let again = state["buffers"][1].clone();
        state["buffers"].as_array_mut().unwrap().push(again);
    });
    assert!(parse(&state).unwrap_err().contains("twice"));
}

fn restore(cursor: usize, top: usize, editor: &Editor) -> Restore {
    Restore {
        buffer: editor.window().buffer(),
        selections: SelectionSet::new(cursor),
        scroll: Scroll { top, left: 0 },
    }
}

#[test]
fn positions_wait_for_the_file_to_load() {
    let mut editor = Editor::default();
    let id = editor.add_buffer(Buffer::visiting(PathBuf::from("/p/a.txt")));
    editor.show(id);
    let restore = restore(6, 0, &editor);
    assert!(!restore.try_apply(&mut editor), "nothing has loaded");
    assert_eq!(editor.window().selections.primary().cursor, 0);

    let text = "abc\ndef\nghi\n";
    let buffer = editor.buffer_mut(id).unwrap();
    buffer.apply_load(decode_all(text.as_bytes(), TabWidth::default()));
    assert!(restore.try_apply(&mut editor));
    assert_eq!(editor.window().selections.primary().cursor, 6);
}

#[test]
fn positions_move_to_what_exists_in_a_changed_file() {
    let mut editor = visiting("/p/a.txt", "日本\n");
    assert!(restore(1, 40, &editor).try_apply(&mut editor));
    assert_eq!(
        editor.window().selections.primary().cursor,
        0,
        "the start of the character"
    );
    assert_eq!(
        editor.window().scroll().top,
        0,
        "scrolled back to the cursor"
    );
    assert!(restore(1000, 0, &editor).try_apply(&mut editor));
    assert_eq!(editor.window().selections.primary().cursor, 7, "the end");
}

#[test]
fn positions_are_dropped_if_the_window_moved_on() {
    let mut editor = visiting("/p/a.txt", "abc\n");
    let restore = restore(2, 0, &editor);
    let (scratch, _) = editor.buffers().next().unwrap();
    editor.show(scratch);
    assert!(restore.try_apply(&mut editor));
    assert_eq!(editor.window().selections.primary().cursor, 0);
}

/// What a save reports of the file it put in place.
fn written() -> FileStamp {
    FileStamp {
        modified: std::time::SystemTime::UNIX_EPOCH,
        len: 0,
        mode: 0o644,
        identity: None,
    }
}

#[test]
fn a_running_save_holds_the_handover_and_a_guarded_exit_by_name() {
    let mut editor = visiting(&absolute("/p/a.txt"), "abc\n");
    let clean = generation(&editor);
    type_text(&mut editor);
    editor.start_save().unwrap();
    let job = editor.take_save_jobs().pop().unwrap();
    // An undo leaves the buffer unmodified, but the disk is about to differ from it.
    editor.undo().unwrap();
    assert!(!editor.current().1.is_modified());
    assert_eq!(generation(&editor), clean);

    for message in [
        check(&editor).unwrap_err(),
        export(&editor, &[], None, None).unwrap_err(),
    ] {
        assert!(message.contains("a.txt"), "{message}");
        assert!(message.contains("being saved"), "{message}");
        assert!(!message.contains(TYPED), "{message}");
    }
    let message = exit_allowed(&editor, Some(clean)).unwrap_err();
    assert!(message.starts_with(EXIT_REFUSED), "{message}");
    assert_eq!(exit_allowed(&editor, None), Ok(()), "a person or an agent");

    editor.save_finished(job.buffer, job.content, written());
    assert!(check(&editor).is_err(), "now modified");
}

#[test]
fn a_finished_save_lets_the_handover_through() {
    let mut editor = visiting(&absolute("/p/a.txt"), "abc\n");
    type_text(&mut editor);
    assert!(check(&editor).is_err());
    editor.start_save().unwrap();
    let job = editor.take_save_jobs().pop().unwrap();
    assert!(check(&editor).is_err());
    editor.save_finished(job.buffer, job.content, written());
    assert!(check(&editor).is_ok());
    assert!(export(&editor, &[], None, None).is_ok());
}

mod secret;

#[test]
fn the_theme_drawn_in_is_handed_over() {
    let mut editor = visiting(&absolute("/p/a.txt"), "abc\n");
    editor.set_themes(vec![("dark", "dark"), ("light", "light")], 1);
    let state = parse(&export(&editor, &[], None, None).unwrap()).unwrap();
    assert_eq!(state.theme.as_deref(), Some("light"));
}

#[test]
fn a_state_with_no_theme_reads_and_one_with_a_bad_name_is_refused() {
    let state = with(|state| {
        state.as_object_mut().unwrap().remove("theme");
    });
    assert_eq!(parse(&state).unwrap().theme, None);
    for bad in [
        json!(""),
        json!("Dark"),
        json!("../x"),
        json!(7),
        json!("a".repeat(33)),
    ] {
        let state = with(|state| state["theme"] = bad.clone());
        let message = parse(&state).unwrap_err();
        assert!(message.contains("theme"), "{message}");
    }
}
