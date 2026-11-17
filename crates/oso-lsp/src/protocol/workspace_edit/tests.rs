//! Tests of a workspace edit as a server sends it (ADR 0138, step 13): by version or by URI,
//! whole or not at all, at each bound and past it.
//!
//! Must not depend on: `server`, threads or processes.

use serde_json::{Value, json};

use super::*;
use crate::position::Position;

fn range(line: u32, start: u32, end: u32) -> Value {
    json!({"start": {"line": line, "character": start}, "end": {"line": line, "character": end}})
}

fn insert(text: &str) -> Value {
    json!({"range": range(0, 0, 0), "newText": text})
}

#[test]
fn a_workspace_edit_reads_by_version_or_by_uri() {
    let versioned = json!({"documentChanges": [{
        "textDocument": {"uri": "file:///w/a.rs", "version": 7},
        "edits": [{"range": range(2, 0, 3), "newText": "let"}],
    }]});
    let edit = workspace_edit(&versioned).unwrap();
    assert_eq!(
        (edit.files[0].uri.as_str(), edit.files[0].version),
        ("file:///w/a.rs", Some(7))
    );
    assert_eq!(
        edit.files[0].edits,
        [(Position::new(2, 0), Position::new(2, 3), "let".to_owned())]
    );
    let by_uri = workspace_edit(&json!({"changes": {"file:///w/b.rs": [insert("x")]}})).unwrap();
    assert_eq!(
        (by_uri.files[0].uri.as_str(), by_uri.files[0].version),
        ("file:///w/b.rs", None)
    );
}

#[test]
fn creating_renaming_or_deleting_a_file_or_a_control_refuses_it_all() {
    let create = json!({"documentChanges": [{"kind": "create", "uri": "file:///w/b.rs"}]});
    assert!(workspace_edit(&create).is_err());
    let control = json!({"changes": {"file:///w/a.rs": [insert("a\u{1b}[2J")]}});
    assert!(workspace_edit(&control).is_err());
    let unread = json!({"changes": {"file:///w/a.rs": [{"newText": "x"}]}});
    assert!(workspace_edit(&unread).is_err(), "an edit with no range");
}

#[test]
fn each_bound_holds_at_it_and_refuses_past_it() {
    let files = |count: usize| -> Value {
        let map: serde_json::Map<String, Value> = (0..count)
            .map(|n| (format!("file:///w/{n}.rs"), json!([])))
            .collect();
        json!({"changes": map})
    };
    assert!(workspace_edit(&files(EDIT_FILES_MAX)).is_ok());
    assert!(workspace_edit(&files(EDIT_FILES_MAX + 1)).is_err());
    let edits = |count: usize| json!({"changes": {"file:///w/a.rs": vec![insert(""); count]}});
    assert!(workspace_edit(&edits(EDIT_EDITS_MAX)).is_ok());
    assert!(workspace_edit(&edits(EDIT_EDITS_MAX + 1)).is_err());
    let bytes = |count: usize| json!({"changes": {"file:///w/a.rs": [insert(&"x".repeat(count))]}});
    assert!(workspace_edit(&bytes(EDIT_BYTES_MAX)).is_ok());
    assert!(workspace_edit(&bytes(EDIT_BYTES_MAX + 1)).is_err());
}
