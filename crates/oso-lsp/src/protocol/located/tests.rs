//! Tests of reading definitions and references: the three shapes, the bound, what is left out,
//! and the requests' parameters.
//!
//! Must not depend on: the session thread; these read values.

use serde_json::{Value, json};

use super::*;

fn location(uri: &str, line: u32) -> Value {
    json!({"uri": uri, "range": {
        "start": {"line": line, "character": 4},
        "end": {"line": line, "character": 9},
    }})
}

#[test]
fn one_location_a_list_and_null_read() {
    let (one, left) = located(&location("file:///a.rs", 3));
    assert_eq!(left, 0);
    assert_eq!(
        one,
        [Located {
            uri: "file:///a.rs".into(),
            start: Position::new(3, 4),
            end: Position::new(3, 9),
        }]
    );
    let (list, _) = located(&json!([
        location("file:///a.rs", 1),
        location("file:///b.rs", 2)
    ]));
    assert_eq!(list.len(), 2);
    assert_eq!(list[1].uri, "file:///b.rs");
    assert_eq!(located(&Value::Null), (Vec::new(), 0));
    assert_eq!(located(&json!([])), (Vec::new(), 0));
}

#[test]
fn a_link_is_read_by_its_target_selection() {
    let link = json!({
        "originSelectionRange": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}},
        "targetUri": "file:///t.rs",
        "targetRange": {"start": {"line": 10, "character": 0}, "end": {"line": 20, "character": 1}},
        "targetSelectionRange": {"start": {"line": 12, "character": 7}, "end": {"line": 12, "character": 11}},
    });
    let (places, _) = located(&json!([link]));
    assert_eq!(places[0].uri, "file:///t.rs");
    assert_eq!(places[0].start, Position::new(12, 7));
    let mut no_selection = link.clone();
    no_selection
        .as_object_mut()
        .unwrap()
        .remove("targetSelectionRange");
    assert_eq!(located(&no_selection).0[0].start, Position::new(10, 0));
}

#[test]
fn what_does_not_read_or_goes_past_the_bound_is_counted() {
    let long = format!("file:///{}", "a".repeat(URI_BYTES_MAX));
    let answer = json!([
        location("file:///a.rs", 1),
        {"uri": "file:///b.rs"},
        {"uri": 7, "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}}},
        location(&long, 2),
        {"uri": "file:///c.rs", "range": {"start": {"line": -1, "character": 0}, "end": {"line": 0, "character": 0}}},
    ]);
    let (places, left) = located(&answer);
    assert_eq!(places.len(), 1);
    assert_eq!(left, 4);
    let many: Vec<Value> = (0..LOCATIONS_MAX as u32 + 5)
        .map(|line| location("file:///a.rs", line))
        .collect();
    let (places, left) = located(&Value::Array(many));
    assert_eq!(places.len(), LOCATIONS_MAX);
    assert_eq!(places.capacity(), LOCATIONS_MAX);
    assert_eq!(left, 5);
}

#[test]
fn the_requests_name_the_document_and_the_position() {
    let params = references_params("file:///a.rs", Position::new(2, 5));
    assert_eq!(params["textDocument"]["uri"], "file:///a.rs");
    assert_eq!(params["position"], json!({"line": 2, "character": 5}));
    assert_eq!(params["context"]["includeDeclaration"], true);
    assert!(
        definition_params("file:///a.rs", Position::new(0, 0))
            .get("context")
            .is_none()
    );
}
