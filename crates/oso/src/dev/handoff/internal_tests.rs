//! The tests of what a state handoff does with internal buffers (ADR 0051, ADR 0063).
//!
//! Must not depend on: a window or the file system; a state is a plain value.

use super::tests::{absolute, visiting};
use super::{
    Restore, check, exit_allowed, export_value as export, generation, import_allowed, parse,
};
use oso_core::buffer::{Buffer, Replacement};
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
    assert_eq!(
        check(&editor),
        Ok(json!({ "generation": before, "unsaved": false }))
    );
    let state = export(&editor, &[], None, None).unwrap();
    assert_eq!(state["generation"], before);
    assert_eq!(state["buffers"].as_array().unwrap().len(), 2);
    assert_eq!(exit_allowed(&editor, Some(before)), Ok(()));
}

#[test]
fn a_window_on_messages_hands_over_where_it_goes_back_to() {
    let mut editor = visiting(&absolute("/p/a.txt"), "abc\ndef\n");
    let at = SelectionSet::new(5);
    let scroll = Scroll { start: 4, left: 0 };
    editor.restore_positions(at.clone(), scroll);
    editor.message("Wrote a.txt");
    editor.toggle_messages();
    assert_eq!(editor.current().1.name(), "*Messages*");
    let state = export(&editor, &[], None, None).unwrap();
    let parsed = parse(&state).unwrap();
    assert_eq!(
        parsed.paths()[parsed.shown],
        Some(PathBuf::from(absolute("/p/a.txt")))
    );
    assert_eq!(parsed.selections, at);
    assert_eq!(parsed.scroll, scroll);
    assert!(!state.to_string().contains("Wrote"));
    assert!(
        !state.to_string().contains("*Messages*"),
        "the buffer is not handed over"
    );
}

#[test]
fn a_window_on_the_emergency_copies_hands_over_where_it_goes_back_to() {
    let mut editor = visiting(&absolute("/p/a.txt"), "abc\ndef\n");
    let at = SelectionSet::new(5);
    let scroll = Scroll { start: 4, left: 0 };
    editor.restore_positions(at.clone(), scroll);
    editor.toggle_copies();
    let state = export(&editor, &[], None, None).unwrap();
    let parsed = parse(&state).unwrap();
    assert_eq!(
        parsed.paths()[parsed.shown],
        Some(PathBuf::from(absolute("/p/a.txt")))
    );
    assert_eq!(parsed.selections, at);
    assert_eq!(parsed.scroll, scroll);
    assert!(!state.to_string().contains("Emergency"));
    assert!(import_allowed(&editor).is_err(), "no *scratch* in view");
}

#[test]
fn a_window_on_messages_with_nothing_behind_it_comes_back_on_scratch() {
    let mut editor = visiting(&absolute("/p/a.txt"), "abc\n");
    let file = editor.window().buffer();
    editor.toggle_messages();
    editor.kill_buffer(file);
    let state = export(&editor, &[], None, None).unwrap();
    let parsed = parse(&state).unwrap();
    assert_eq!(parsed.paths()[parsed.shown], None, "*scratch*");
    assert_eq!(parsed.selections, SelectionSet::default());
    assert_eq!(parsed.scroll, Scroll::default());
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
fn a_secret_behind_messages_is_handed_over_unshown_with_no_position() {
    let path = PathBuf::from(absolute("/p/.env"));
    let mut buffer = Buffer::visiting(path.clone());
    buffer.apply_load(decode_all(b"KEY=value\n", TabWidth::default()));
    buffer.judge_secret(Some(std::slice::from_ref(&path)));
    let mut editor = Editor::with_buffer(buffer);
    editor.current_mut().0.selections = SelectionSet::new(4);
    editor.toggle_messages();
    let state = export(&editor, &[], None, None).unwrap();
    let parsed = parse(&state).unwrap();
    assert_eq!(
        parsed.paths()[parsed.shown],
        None,
        "the window comes back on *scratch*"
    );
    assert!(
        parsed.paths().contains(&Some(path)),
        "it can still be reached"
    );
    assert_eq!(parsed.selections, SelectionSet::default());
    let shown = crate::dev::get_state(&editor, &[], None);
    assert_eq!(shown["buffer"]["name"], "*Messages*");
    assert_eq!(shown["cursor"]["offset"], 0, "*Messages* is empty");
    assert_eq!(shown["windows"][0]["selections"][0]["cursor"], 0);
    editor.toggle_messages();
    assert_eq!(editor.window().selections.primary().cursor, 4);
}

#[test]
fn a_modified_file_behind_messages_is_carried() {
    let mut editor = visiting(&absolute("/p/a.txt"), "abc\n");
    let id = editor.window().buffer();
    let edit = Replacement {
        range: 0..0,
        text: "x",
    };
    editor.apply_edits(id, &[edit]).unwrap();
    editor.toggle_messages();
    assert!(check(&editor).is_ok());
    let state = export(&editor, &[], None, None).unwrap();
    let parsed = parse(&state).unwrap();
    assert!(parsed.buffers[parsed.shown].carried.is_some());
}

#[test]
fn positions_still_waiting_behind_messages_are_the_imported_ones() {
    let path = PathBuf::from(absolute("/p/a.txt"));
    let mut buffer = Buffer::visiting(path.clone());
    buffer.apply_load(decode_all(b"KEY=value\n", TabWidth::default()));
    buffer.judge_secret(Some(std::slice::from_ref(&path)));
    let mut editor = Editor::with_buffer(buffer);
    let id = editor.window().buffer();
    editor.current_mut().0.selections = SelectionSet::new(4);
    editor.toggle_messages();
    let restore = Restore {
        buffer: id,
        selections: SelectionSet::new(2),
        scroll: Scroll::default(),
        top_is_row: false,
    };
    let state = export(&editor, &[], None, Some(&restore)).unwrap();
    assert_eq!(state["windows"][0]["selections"][0]["cursor"], 2);
}

#[test]
fn a_composition_holds_the_handover_and_an_exit_for_a_generation() {
    let mut editor = Editor::default();
    let before = generation(&editor);
    let preedit = oso_core::display_map::Preedit::new("にほん".into(), None);
    editor.set_preedit(Some(preedit));
    assert_eq!(
        check(&editor),
        Err("cannot hand over: a composition is open".into())
    );
    let refused = exit_allowed(&editor, Some(before)).unwrap_err();
    assert!(refused.ends_with("a composition is open"), "{refused}");
    assert_eq!(
        exit_allowed(&editor, None),
        Ok(()),
        "asked plainly, it exits"
    );
    editor.set_preedit(None);
    assert_eq!(
        check(&editor),
        Ok(json!({ "generation": before, "unsaved": false }))
    );
}

#[test]
fn a_search_holds_the_handover_and_an_exit_for_a_generation() {
    let mut editor = Editor::default();
    let before = generation(&editor);
    editor.isearch_begin(false);
    assert_eq!(
        check(&editor),
        Err("cannot hand over: a search is under way".into())
    );
    assert!(export(&editor, &[], None, None).is_err());
    let refused = exit_allowed(&editor, Some(before)).unwrap_err();
    assert!(refused.ends_with("a search is under way"), "{refused}");
    editor.isearch_exit();
    assert_eq!(
        check(&editor),
        Ok(json!({ "generation": before, "unsaved": false }))
    );
}

#[test]
fn a_split_frame_holds_the_handover_until_the_state_carries_its_layout() {
    let mut editor = Editor::default();
    editor.set_frame_size(oso_core::frame::FrameSize {
        rows: 24,
        columns: 80,
    });
    let before = generation(&editor);
    editor
        .split_window(oso_core::frame::Way::Down, false)
        .unwrap();
    let refused = "cannot hand over: the frame has more than one window";
    assert_eq!(check(&editor), Err(refused.into()));
    assert_eq!(export(&editor, &[], None, None), Err(refused.into()));
    assert!(exit_allowed(&editor, Some(before)).is_err());
    editor.delete_other_windows();
    assert_eq!(
        check(&editor),
        Ok(json!({ "generation": before, "unsaved": false }))
    );
}
