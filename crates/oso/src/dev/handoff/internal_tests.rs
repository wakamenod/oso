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
        parsed.paths()[parsed.window().shown],
        Some(PathBuf::from(absolute("/p/a.txt")))
    );
    assert_eq!(parsed.window().selections, at);
    assert_eq!(parsed.window().scroll, scroll);
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
        parsed.paths()[parsed.window().shown],
        Some(PathBuf::from(absolute("/p/a.txt")))
    );
    assert_eq!(parsed.window().selections, at);
    assert_eq!(parsed.window().scroll, scroll);
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
    assert_eq!(parsed.paths()[parsed.window().shown], None, "*scratch*");
    assert_eq!(parsed.window().selections, SelectionSet::default());
    assert_eq!(parsed.window().scroll, Scroll::default());
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
        parsed.paths()[parsed.window().shown],
        None,
        "the window comes back on *scratch*"
    );
    assert!(
        parsed.paths().contains(&Some(path)),
        "it can still be reached"
    );
    assert_eq!(parsed.window().selections, SelectionSet::default());
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
    assert!(parsed.buffers[parsed.window().shown].carried.is_some());
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
        window: editor.window().id(),
        buffer: id,
        selections: SelectionSet::new(2),
        scroll: Scroll::default(),
        top_is_row: false,
        untouched: None,
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
fn a_query_replace_holds_the_handover_and_an_exit_for_a_generation() {
    let mut editor = Editor::default();
    let id = editor.window().buffer();
    let text = oso_core::buffer::Replacement {
        range: 0..0,
        text: "a a",
    };
    editor.apply_edits(id, &[text]).unwrap();
    editor.current_mut().0.selections = oso_core::selection::SelectionSet::new(0);
    let before = generation(&editor);
    editor.replace_begin("a", "b", false).unwrap();
    assert!(editor.replace().is_some());
    assert_eq!(
        check(&editor),
        Err("cannot hand over: a search is under way".into())
    );
    let refused = exit_allowed(&editor, Some(before)).unwrap_err();
    assert!(refused.ends_with("a search is under way"), "{refused}");
    editor.replace_exit();
    assert!(check(&editor).is_ok());
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

/// An editor on `*scratch*` holding `text`, which a handoff carries.
fn scratch_with(text: &str) -> Editor {
    let mut editor = Editor::default();
    let id = editor.window().buffer();
    let edit = Replacement { range: 0..0, text };
    editor.apply_edits(id, &[edit]).unwrap();
    editor
}

#[test]
fn a_split_frame_hands_over_every_window_in_the_trees_order_with_its_layout() {
    let mut editor = scratch_with("one\ntwo\nthree\n");
    editor.set_frame_size(oso_core::frame::FrameSize {
        rows: 24,
        columns: 81,
    });
    editor
        .split_window(oso_core::frame::Way::Across, false)
        .unwrap();
    editor
        .split_window(oso_core::frame::Way::Down, true)
        .unwrap();
    editor.current_mut().0.selections = SelectionSet::new(4);
    assert!(check(&editor).is_ok(), "a split frame hands over");
    let state = export(&editor, &[], None, None).unwrap();
    assert_eq!(state["version"], 4);
    assert_eq!(
        state["layout"],
        json!({ "across": [
            { "weight": 41, "run": { "down": [
                { "weight": 12, "window": 0 },
                { "weight": 11, "window": 1 },
            ] } },
            { "weight": 40, "window": 2 },
        ] })
    );
    assert_eq!(state["selected"], 1, "the left one split below, selected");
    let parsed = parse(&state).unwrap();
    assert_eq!(parsed.windows.windows.len(), 3);
    assert_eq!(parsed.windows.layout, editor.tiling());
    assert_eq!(parsed.windows.selected, 1);
    assert_eq!(parsed.windows.windows[1].selections, SelectionSet::new(4));
    // The right one kept the cursor where the text was typed.
    assert_eq!(parsed.windows.windows[2].selections, SelectionSet::new(14));
}

#[test]
fn a_state_of_version_3_reads_as_one_window() {
    let editor = scratch_with("one\n");
    let mut state = export(&editor, &[], None, None).unwrap();
    state["version"] = json!(3);
    state.as_object_mut().unwrap().remove("layout");
    let parsed = parse(&state).unwrap();
    assert_eq!(parsed.windows.layout, oso_core::frame::Tiling::Window);
    assert_eq!(parsed.windows.windows.len(), 1);
    state["windows"] = json!([state["windows"][0], state["windows"][0]]);
    assert!(parse(&state).unwrap_err().contains("one window"));
}

#[test]
fn windows_split_deeper_than_a_state_keeps_hold_the_handover_with_a_reason() {
    let mut editor = scratch_with("deep\n");
    editor.set_frame_size(oso_core::frame::FrameSize {
        rows: 2000,
        columns: 4000,
    });
    for split in 0..=oso_core::frame::Tiling::MAX_DEPTH {
        let way = if split % 2 == 0 {
            oso_core::frame::Way::Down
        } else {
            oso_core::frame::Way::Across
        };
        editor.split_window(way, true).unwrap();
    }
    assert!(editor.tiling().depth() > oso_core::frame::Tiling::MAX_DEPTH);
    let refused = "cannot hand over: the windows are split more than 16 deep";
    assert_eq!(check(&editor), Err(refused.into()));
    assert_eq!(export(&editor, &[], None, None), Err(refused.into()));
    editor.delete_other_windows();
    assert!(check(&editor).is_ok());
}

#[test]
fn more_windows_than_a_state_keeps_hold_the_handover_with_a_reason() {
    let mut editor = scratch_with("many\n");
    editor.set_frame_size(oso_core::frame::FrameSize {
        rows: 40,
        columns: 4000,
    });
    let children = (0..257).map(|_| (1, oso_core::frame::Tiling::Window));
    let tiling = oso_core::frame::Tiling::Run {
        way: oso_core::frame::Way::Across,
        children: children.collect(),
    };
    assert_eq!(editor.set_tiling(&tiling).unwrap().len(), 257);
    let refused = "cannot hand over: the frame has more than 256 windows";
    assert_eq!(check(&editor), Err(refused.into()));
    assert_eq!(export(&editor, &[], None, None), Err(refused.into()));
    editor.delete_window().unwrap();
    assert!(check(&editor).is_ok());
}

#[test]
fn positions_wait_in_a_window_not_selected_without_selecting_it_and_go_once_it_moves() {
    let mut editor = scratch_with(&"line\n".repeat(50));
    editor.set_frame_size(oso_core::frame::FrameSize {
        rows: 24,
        columns: 80,
    });
    let me = editor.window().id();
    let other = editor
        .split_window(oso_core::frame::Way::Down, false)
        .unwrap();
    let buffer = editor.window().buffer();
    let untouched = editor
        .window_by_id(other)
        .map(|window| (window.selections.clone(), window.scroll()));
    let restore = Restore {
        window: other,
        buffer,
        selections: SelectionSet::new(25),
        scroll: Scroll { start: 10, left: 0 },
        top_is_row: false,
        untouched: untouched.clone(),
    };
    assert!(restore.stands(&editor));
    assert!(restore.try_apply(&mut editor));
    assert_eq!(editor.window().id(), me, "not selected for it");
    let window = editor.window_by_id(other).unwrap();
    assert_eq!(window.selections.primary().cursor, 25);

    // Moved meanwhile, it keeps where the person put it.
    let late = Restore {
        selections: SelectionSet::new(40),
        untouched,
        ..restore
    };
    assert!(!late.stands(&editor));
    assert!(late.try_apply(&mut editor), "over: dropped");
    let window = editor.window_by_id(other).unwrap();
    assert_eq!(window.selections.primary().cursor, 25);
}

#[test]
fn positions_wait_behind_another_buffer_only_while_the_place_kept_there_is_untouched() {
    let waiting = |editor: &Editor| Restore {
        window: editor.window().id(),
        buffer: editor.window().buffer(),
        selections: SelectionSet::new(25),
        scroll: Scroll { start: 10, left: 0 },
        top_is_row: false,
        untouched: Some((editor.window().selections.clone(), editor.window().scroll())),
    };
    // Gone to another buffer untouched: the place kept for this one takes them.
    let mut editor = scratch_with(&"line\n".repeat(50));
    let restore = waiting(&editor);
    editor.toggle_messages();
    assert!(restore.stands(&editor));
    assert!(restore.try_apply(&mut editor));
    editor.toggle_messages();
    assert_eq!(editor.window().selections.primary().cursor, 25);

    // Moved by a person, then gone to another buffer: where they left it stays.
    let mut editor = scratch_with(&"line\n".repeat(50));
    let restore = waiting(&editor);
    editor.restore_positions(SelectionSet::new(5), Scroll::default());
    editor.toggle_messages();
    assert!(!restore.stands(&editor));
    assert!(restore.try_apply(&mut editor), "over: dropped");
    editor.toggle_messages();
    assert_eq!(editor.window().selections.primary().cursor, 5);
}
