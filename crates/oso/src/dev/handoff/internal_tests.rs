//! The tests of what a state handoff does with internal buffers (ADR 0051, ADR 0063).
//!
//! Must not depend on: a window or the file system; a state is a plain value.

use super::tests::{absolute, visiting};
use super::{check, exit_allowed, export, generation, import_allowed, parse};
use oso_core::buffer::Buffer;
use oso_core::display_map::TabWidth;
use oso_core::editor::Editor;
use oso_core::load::decode_all;
use oso_core::selection::SelectionSet;
use oso_core::window::Scroll;
use serde_json::json;
use std::path::PathBuf;

#[test]
fn messages_hold_no_restart_and_change_no_generation() {
    let mut editor = visiting(&absolute("/p/a.txt"), "abc\n");
    let before = generation(&editor);
    editor.message("Wrote a.txt");
    assert_eq!(generation(&editor), before);
    assert_eq!(check(&editor), Ok(json!({ "generation": before })));
    let state = export(&editor, &[], None, None).unwrap();
    assert_eq!(state["generation"], before);
    assert_eq!(state["buffers"].as_array().unwrap().len(), 2);
    assert_eq!(exit_allowed(&editor, Some(before)), Ok(()));
}

#[test]
fn a_window_on_messages_comes_back_on_scratch() {
    let mut editor = visiting(&absolute("/p/a.txt"), "abc\n");
    editor.message("Wrote a.txt");
    editor.toggle_messages();
    assert_eq!(editor.current().1.name(), "*Messages*");
    let state = export(&editor, &[], None, None).unwrap();
    let parsed = parse(&state).unwrap();
    assert_eq!(parsed.buffers[parsed.shown], None, "*scratch*");
    assert_eq!(parsed.selections, SelectionSet::default());
    assert_eq!(parsed.scroll, Scroll::default());
    assert!(
        parsed
            .buffers
            .contains(&Some(PathBuf::from(absolute("/p/a.txt")))),
        "the file is still handed over"
    );
    assert!(!state.to_string().contains("Wrote"));
}

#[test]
fn an_instance_with_messages_may_still_take_over() {
    let mut editor = Editor::default();
    editor.message("No emergency copy can be written if oso crashes");
    assert_eq!(import_allowed(&editor), Ok(()));
}

#[test]
fn an_instance_with_its_window_on_messages_refuses_to_take_over() {
    let mut editor = Editor::default();
    editor.toggle_messages();
    assert!(import_allowed(&editor).is_err());
}

#[test]
fn a_secret_behind_messages_gives_no_position() {
    let path = PathBuf::from(absolute("/p/.env"));
    let mut buffer = Buffer::visiting(path.clone());
    buffer.apply_load(decode_all(b"KEY=value\n", TabWidth::default()));
    buffer.judge_secret(Some(&[path]));
    let mut editor = Editor::with_buffer(buffer);
    editor.current_mut().0.selections = SelectionSet::new(4);
    editor.toggle_messages();
    let state = export(&editor, &[], None, None).unwrap();
    assert_eq!(state["windows"][0]["selections"][0]["cursor"], 0);
    let shown = crate::dev::get_state(&editor, &[], None);
    assert_eq!(shown["buffer"]["name"], "*Messages*");
    assert!(!shown.to_string().contains("\"offset\":4"));
    editor.toggle_messages();
    assert_eq!(editor.window().selections.primary().cursor, 4);
}
