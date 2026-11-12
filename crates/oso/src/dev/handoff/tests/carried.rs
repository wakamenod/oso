//! The tests of what the state handoff carries (ADR 0090): the text of modified buffers,
//! what was known of their files, marks and the region, kills and histories, and that a
//! state two versions old is refused.
//!
//! Must not depend on: a window or the file system; a state is a plain value.

use super::super::carry::MAX_CARRIED;
use super::super::{Restore, check, export_value as export, generation, parse};
use super::{Mutation, TYPED, absolute, restore, type_text, visiting};
use oso_core::buffer::{Buffer, FileStamp, Replacement};
use oso_core::display_map::TabWidth;
use oso_core::editor::Editor;
use oso_core::load::decode_all;
use oso_core::selection::{Selection, SelectionSet};
use oso_core::window::Scroll;
use serde_json::{Value, json};
use std::path::PathBuf;

#[test]
fn a_modified_file_is_carried_with_what_was_known_of_it_and_no_text_in_the_state() {
    let mut editor = visiting(&absolute("/p/notes.txt"), "on disk\n");
    let id = editor.window().buffer();
    let facts = editor.buffer(id).unwrap().file_facts();
    assert_eq!(check(&editor).unwrap()["unsaved"], false);
    type_text(&mut editor);
    assert_eq!(
        check(&editor).unwrap()["unsaved"],
        true,
        "it may hand over, not be closed"
    );
    let (state, exported) = super::super::export(&editor, &[], None, &[], 7).unwrap();
    assert!(
        !state.to_string().contains(TYPED),
        "the state names lengths only"
    );
    assert_eq!(state["export"], 7);
    let text = format!("{TYPED}on disk\n");
    assert_eq!(state["items"], json!([text.len()]));
    let parsed = parse(&state).unwrap();
    let carried = parsed.buffers[parsed.window().shown]
        .carried
        .as_ref()
        .unwrap();
    assert_eq!(carried.item, 0);
    assert_eq!(carried.facts, facts);
    let piece = exported.piece(7, 0, 0).unwrap().run().unwrap();
    assert_eq!(piece["text"], text);
    assert_eq!(piece["end"], text.len());
}

#[test]
fn text_in_scratch_is_carried_and_an_empty_scratch_needs_nothing() {
    let mut editor = Editor::default();
    let empty = export(&editor, &[], None, None).unwrap();
    assert!(empty["buffers"][0]["text"].is_null());
    assert_eq!(empty["items"], json!([]));
    type_text(&mut editor);
    assert!(check(&editor).is_ok());
    let state = export(&editor, &[], None, None).unwrap();
    assert!(state["buffers"][0]["path"].is_null());
    assert_eq!(state["buffers"][0]["text"]["item"], 0);
    assert_eq!(state["items"], json!([TYPED.len()]));
    // Emptied again, it holds nothing to carry.
    let id = editor.window().buffer();
    let all = Replacement {
        range: 0..TYPED.len(),
        text: "",
    };
    editor.apply_edits(id, &[all]).unwrap();
    assert!(export(&editor, &[], None, None).unwrap()["buffers"][0]["text"].is_null());
}

#[test]
fn a_save_or_an_undo_changes_the_generation_as_the_buffer_stops_being_modified() {
    let mut editor = visiting(&absolute("/p/a.txt"), "abc\n");
    let clean = generation(&editor);
    type_text(&mut editor);
    let edited = generation(&editor);
    assert_ne!(edited, clean);
    let (_, buffer) = editor.current();
    let content = buffer.content_id();
    let id = editor.window().buffer();
    editor.buffer_mut(id).unwrap().mark_saved(content);
    assert_ne!(
        generation(&editor),
        edited,
        "a save between export and exit counts"
    );
    let state = export(&editor, &[], None, None).unwrap();
    assert!(
        state["buffers"][1]["text"].is_null(),
        "read again from disk"
    );
}

#[test]
fn a_kill_changes_the_generation_and_is_carried_without_secret_or_clipboard_ones() {
    use oso_core::kill::Piece;
    use oso_core::rope::Rope;
    let mut editor = visiting(&absolute("/p/a.txt"), "abc\n");
    let before = generation(&editor);
    editor.carry_kills(vec![vec![
        Piece::from(Rope::from("one")),
        Piece::from(Rope::from("two")),
    ]]);
    assert_ne!(generation(&editor), before);
    let state = export(&editor, &[], None, None).unwrap();
    assert_eq!(state["kills"], json!([{ "item": 0, "pieces": [3, 3] }]));
    assert_eq!(state["items"], json!([6]));
    let parsed = parse(&state).unwrap();
    assert_eq!(parsed.kills[0].pieces, [3, 3]);
}

#[test]
fn marks_the_region_and_the_mark_ring_round_trip_and_are_put_back() {
    use oso_core::selection::{MarkActive, MarkRing};
    let mut editor = visiting(&absolute("/p/a.txt"), &"line\n".repeat(10));
    let selections = vec![
        Selection {
            cursor: 2,
            mark: Some(0),
            goal: None,
        },
        Selection {
            cursor: 12,
            mark: Some(7),
            goal: None,
        },
    ];
    let mut set = SelectionSet::from_parts(selections.clone(), 1).unwrap();
    set.set_mark_active(MarkActive::Active);
    let set = set.with_ring(MarkRing::from_newest([20, 3]));
    editor.restore_positions(set.clone(), Scroll::default());
    let state = parse(&export(&editor, &[], None, None).unwrap()).unwrap();
    assert_eq!(state.window().selections, set);

    let mut new = visiting(&absolute("/p/a.txt"), &"line\n".repeat(10));
    let restore = Restore {
        window: new.window().id(),
        buffer: new.window().buffer(),
        selections: state.window().selections.clone(),
        scroll: state.window().scroll,
        top_is_row: false,
        untouched: None,
    };
    assert!(restore.try_apply(&mut new));
    assert_eq!(
        new.window().selections,
        set,
        "a region shown comes back shown"
    );
}

#[test]
fn marks_and_the_ring_move_to_what_exists_and_wait_for_the_file() {
    use oso_core::selection::MarkRing;
    let mut editor = Editor::default();
    let id = editor.add_buffer(Buffer::visiting(PathBuf::from("/p/a.txt")));
    editor.show(id);
    let selections = vec![Selection {
        cursor: 0,
        mark: Some(4),
        goal: None,
    }];
    let restore = Restore {
        window: editor.window().id(),
        buffer: id,
        selections: SelectionSet::from_parts(selections, 0)
            .unwrap()
            .with_ring(MarkRing::from_newest([1, 99])),
        scroll: Scroll::default(),
        top_is_row: false,
        untouched: None,
    };
    let buffer = editor.buffer_mut(id).unwrap();
    let mut builder = oso_core::load::Builder::new(None, TabWidth::default());
    builder.push(b"\xe6\x97\xa5");
    buffer.apply_load(builder.snapshot());
    assert!(
        !restore.try_apply(&mut editor),
        "the mark is past what has loaded"
    );
    let buffer = editor.buffer_mut(id).unwrap();
    buffer.apply_load(decode_all("日本".as_bytes(), TabWidth::default()));
    assert!(restore.try_apply(&mut editor));
    let window = &editor.window().selections;
    assert_eq!(window.primary().mark, Some(3), "the start of the character");
    assert_eq!(window.mark_ring().iter().collect::<Vec<_>>(), [0, 6]);
}

#[test]
fn positions_wait_behind_messages_for_their_file_and_are_there_on_the_way_back() {
    let mut editor = Editor::default();
    let id = editor.add_buffer(Buffer::visiting(PathBuf::from("/p/a.txt")));
    editor.show(id);
    let restore = restore(6, 1, &editor);
    editor.toggle_messages();
    assert!(
        !restore.try_apply(&mut editor),
        "it waits behind *Messages*"
    );
    let text = "abc\ndef\nghi\n";
    let buffer = editor.buffer_mut(id).unwrap();
    buffer.apply_load(decode_all(text.as_bytes(), TabWidth::default()));
    assert!(restore.try_apply(&mut editor));
    editor.toggle_messages();
    assert_eq!(editor.window().buffer(), id);
    assert_eq!(editor.window().selections.primary().cursor, 6);
}

#[test]
fn a_state_two_versions_old_is_refused() {
    let state = json!({
        "version": 1,
        "generation": 3,
        "buffers": [{ "id": 0, "path": null }],
        "windows": [{
            "buffer": 0,
            "primary": 0,
            "selections": [{ "cursor": 0, "goal": null }],
            "scroll": { "top": 0, "left": 0 },
        }],
        "selected": 0,
        "pending_keys": "",
        "frame": null,
    });
    let message = parse(&state).unwrap_err();
    assert!(message.contains("version 1"), "{message}");
}

/// A state that carries a modified file, a kill and a history.
fn carrying(mutate: impl FnOnce(&mut Value)) -> Value {
    use oso_core::kill::Piece;
    use oso_core::rope::Rope;
    let mut editor = visiting(&absolute("/p/a.txt"), "abc\n");
    type_text(&mut editor);
    editor.carry_kills(vec![vec![
        Piece::from(Rope::from("k1")),
        Piece::from(Rope::from("k2")),
    ]]);
    editor.carry_history("line", ["12".to_owned()]);
    let mut state = export(&editor, &[], None, None).unwrap();
    assert!(parse(&state).is_ok());
    mutate(&mut state);
    state
}

#[test]
fn what_was_known_of_a_carried_file_round_trips() {
    use oso_core::buffer::Baseline;
    use std::time::{Duration, SystemTime};
    let mut editor = visiting(&absolute("/p/a.txt"), "abc\r\n");
    let stamp = |modified| FileStamp {
        identity: Some((7, 42)),
        modified,
        len: 5,
        mode: 0o600,
    };
    let id = editor.window().buffer();
    let buffer = editor.buffer_mut(id).unwrap();
    buffer.set_baseline(Baseline {
        location: PathBuf::from(absolute("/real/a.txt")),
        file: Some(stamp(SystemTime::UNIX_EPOCH - Duration::new(3, 5))),
    });
    buffer.set_own_spelling(PathBuf::from(absolute("/real/A.txt")));
    let facts = buffer.file_facts();
    type_text(&mut editor);
    let state = parse(&export(&editor, &[], None, None).unwrap()).unwrap();
    let carried = state.buffers[state.window().shown]
        .carried
        .as_ref()
        .unwrap();
    assert_eq!(carried.facts, facts);
    assert_eq!(facts.info.line_ending, oso_core::decode::LineEnding::Crlf);
}

#[test]
fn bad_carried_fields_are_refused() {
    let bad: [(&str, Mutation); 11] = [
        ("version 5", Box::new(|s| s["version"] = json!(5))),
        (
            "no items",
            Box::new(|s| {
                s.as_object_mut()
                    .unwrap()
                    .remove("items")
                    .map(drop)
                    .unwrap()
            }),
        ),
        ("no item 9", Box::new(|s| s["kills"][0]["item"] = json!(9))),
        (
            "twice",
            Box::new(|s| s["histories"][0]["item"] = s["kills"][0]["item"].clone()),
        ),
        (
            "add up",
            Box::new(|s| s["kills"][0]["pieces"] = json!([1, 1])),
        ),
        (
            "no pieces",
            Box::new(|s| s["kills"][0]["pieces"] = json!([])),
        ),
        (
            "more than",
            Box::new(|s| s["items"][0] = json!(MAX_CARRIED + 1)),
        ),
        (
            "line ending",
            Box::new(|s| s["buffers"][1]["text"]["line_ending"] = json!("cr")),
        ),
        (
            "nanos",
            Box::new(|s| {
                s["buffers"][1]["text"]["baseline"] = json!({
                    "location": absolute("/p/a.txt"),
                    "file": { "secs": 0, "nanos": 1_000_000_000, "before_epoch": false, "len": 0, "mode": 0 },
                });
            }),
        ),
        (
            "absolute",
            Box::new(|s| s["buffers"][1]["text"]["own_spelling"] = json!("a.txt")),
        ),
        (
            "history",
            Box::new(|s| s["histories"][0]["lengths"] = json!(vec![0; 101])),
        ),
    ];
    for (expected, mutate) in bad {
        let state = carrying(|state| mutate(state));
        let message = parse(&state).unwrap_err();
        assert!(message.contains(expected), "{expected}: {message}");
    }
}

#[test]
fn piece_lengths_that_overflow_are_refused_without_a_panic() {
    let state = carrying(|s| s["kills"][0]["pieces"] = json!([u64::MAX, 2]));
    assert!(parse(&state).unwrap_err().contains("add up"));
}

#[test]
fn a_buffer_the_agent_opened_says_so_read_again_or_carried() {
    let path = absolute("/p/opened.txt");
    let mut editor = visiting(&path, "abc\n");
    let id = editor.window().buffer();
    let entry = |editor: &Editor| export(editor, &[], None, None).unwrap()["buffers"][1].clone();
    assert_eq!(
        entry(&editor).get("opened_by_agent"),
        None,
        "said only when true"
    );
    editor.buffer_mut(id).unwrap().mark_opened_by_agent();
    let read_again = export(&editor, &[], None, None).unwrap();
    assert_eq!(read_again["buffers"][1]["opened_by_agent"], true);
    let parsed = parse(&read_again).unwrap();
    assert!(parsed.buffers[1].carried.is_none());
    assert!(parsed.buffers[1].opened_by_agent);
    type_text(&mut editor);
    let carried = parse(&export(&editor, &[], None, None).unwrap()).unwrap();
    assert!(carried.buffers[1].carried.is_some());
    assert!(carried.buffers[1].opened_by_agent);
    assert!(!carried.buffers[0].opened_by_agent, "*scratch* was not");
}

#[test]
fn an_entry_that_does_not_say_was_not_opened_by_the_agent_and_a_bad_value_is_refused() {
    let state = carrying(|state| {
        state["buffers"][1]
            .as_object_mut()
            .unwrap()
            .remove("opened_by_agent");
    });
    assert!(!parse(&state).unwrap().buffers[1].opened_by_agent);
    let bad = carrying(|state| state["buffers"][1]["opened_by_agent"] = json!("yes"));
    assert!(parse(&bad).unwrap_err().contains("opened_by_agent"));
}

#[test]
fn a_file_the_agent_is_still_opening_is_not_handed_over() {
    let path = absolute("/p/opened.txt");
    let mut editor = visiting(&absolute("/p/notes.txt"), "abc\n");
    let mut opening = Buffer::visiting(PathBuf::from(&path));
    opening.set_agent_pending(true);
    editor.add_buffer(opening);
    let state = export(&editor, &[], None, None).unwrap();
    let paths: Vec<&Value> = state["buffers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|buffer| &buffer["path"])
        .collect();
    assert!(!paths.contains(&&json!(path)), "{paths:?}");
    assert!(paths.contains(&&json!(absolute("/p/notes.txt"))));
}
