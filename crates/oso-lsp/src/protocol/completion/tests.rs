//! Tests of reading a completion answer (ADR 0138, step 9): its two shapes, the order by
//! `sortText`, the bounds, the edit's range of either kind, and snippets as plain text.
//!
//! Must not depend on: `server`, threads or processes.

use proptest::prelude::*;
use serde_json::{Value, json};

use super::*;
use crate::limits::{ADDITIONAL_EDITS_BYTES_MAX, ADDITIONAL_EDITS_MAX};

#[test]
fn a_list_or_an_array_is_read_in_the_order_of_its_sort_text() {
    let answer = json!({"isIncomplete": true, "items": [
        {"label": "zeta", "sortText": "1", "kind": 3, "detail": "fn zeta()"},
        {"label": "alpha", "sortText": "2", "filterText": "al"},
        {"label": "beta"},
    ]});
    let completed = completed(&answer);
    assert!(completed.incomplete);
    let labels: Vec<_> = completed
        .items
        .iter()
        .map(|item| item.label.as_str())
        .collect();
    assert_eq!(labels, ["zeta", "alpha", "beta"]);
    assert_eq!(completed.items[0].kind, Some(3));
    assert_eq!(completed.items[0].detail, "fn zeta()");
    assert_eq!(completed.items[1].filter.as_deref(), Some("al"));
    assert_eq!(
        completed.items[2].text, "beta",
        "the label, with nothing else to insert"
    );
    let array = completed_of(json!([{"label": "x", "insertText": "x()"}]));
    assert!(!array.incomplete);
    assert_eq!(array.items[0].text, "x()");
    assert_eq!(completed_of(json!(null)), CompletionList::default());
}

fn completed_of(value: serde_json::Value) -> CompletionList {
    completed(&value)
}

#[test]
fn an_edit_is_read_by_its_range_or_its_insert_range() {
    let range = json!({"start": {"line": 1, "character": 4}, "end": {"line": 1, "character": 6}});
    let answer = json!([
        {"label": "a", "textEdit": {"range": range, "newText": "apple"}},
        {"label": "b", "textEdit": {"insert": range, "replace": range, "newText": "banana"}},
        {"label": "c", "textEdit": {"newText": "lost"}},
    ]);
    let completed = completed(&answer);
    let at = Some((Position::new(1, 4), Position::new(1, 6)));
    assert_eq!(completed.items.len(), 2, "an edit with no range is dropped");
    assert_eq!(
        (completed.items[0].range, completed.items[0].text.as_str()),
        (at, "apple")
    );
    assert_eq!(
        (completed.items[1].range, completed.items[1].text.as_str()),
        (at, "banana")
    );
    assert_eq!(completed.left, 1);
}

#[test]
fn bounds_cut_what_shows_and_drop_what_would_insert_too_much() {
    let long = "é".repeat(LABEL_BYTES_MAX);
    let big = "x".repeat(INSERT_TEXT_BYTES_MAX + 1);
    let mut items = vec![
        json!({"label": long, "detail": long}),
        json!({"label": "big", "insertText": big}),
    ];
    items.extend((0..COMPLETION_ITEMS_MAX + 5).map(|n| json!({"label": format!("n{n}")})));
    let completed = completed(&json!(items));
    assert!(completed.items[0].label.len() <= LABEL_BYTES_MAX);
    assert!(completed.items.iter().all(|item| item.label != "big"));
    assert_eq!(completed.items.len(), COMPLETION_ITEMS_MAX - 1);
    assert_eq!(completed.left, 8, "7 past the bound and the one too big");
}

#[test]
fn a_snippet_is_read_as_plain_text() {
    assert_eq!(plain("foo(${1:a}, ${2:b})$0"), "foo(a, b)");
    assert_eq!(plain("x$1 ${3|one,two|} \\$y"), "x one $y");
    assert_eq!(plain("${TM_SELECTED_TEXT:sel} $HOME!"), "sel !");
    assert_eq!(plain("${1:outer ${2:inner}}"), "outer inner");
    let answer = json!([{"label": "f", "insertText": "f(${1:x})", "insertTextFormat": 2}]);
    assert_eq!(completed(&answer).items[0].text, "f(x)");
}

#[test]
fn the_request_asks_as_the_person_invoked_it() {
    let params = completion_params("file:///a.rs", Position::new(2, 5), Invoked::Asked);
    assert_eq!(params["context"]["triggerKind"], 1);
    let typed = completion_params("file:///a.rs", Position::new(2, 5), Invoked::Character('.'));
    assert_eq!(
        typed["context"],
        json!({"triggerKind": 2, "triggerCharacter": "."})
    );
    let again = completion_params("file:///a.rs", Position::new(2, 5), Invoked::Incomplete);
    assert_eq!(again["context"]["triggerKind"], 3);
    assert_eq!(params["position"]["character"], 5);
    assert_eq!(kind_name(22), "struct");
    assert_eq!(kind_name(99), "");
}

#[test]
fn a_range_is_kept_only_on_the_asked_line_and_around_the_asked_place() {
    let at = Position::new(4, 6);
    let offered = |start: (u32, u32), end: (u32, u32)| Offered {
        label: "x".into(),
        kind: None,
        detail: String::new(),
        filter: None,
        range: Some((Position::new(start.0, start.1), Position::new(end.0, end.1))),
        text: "x".into(),
        extra: Vec::new(),
    };
    assert!(offered((4, 2), (4, 6)).range_around(at).is_some());
    assert!(offered((4, 6), (4, 9)).range_around(at).is_some());
    assert!(
        offered((0, 0), (4, 6)).range_around(at).is_none(),
        "from another line"
    );
    assert!(
        offered((4, 2), (9, 0)).range_around(at).is_none(),
        "to another line"
    );
    assert!(
        offered((4, 7), (4, 9)).range_around(at).is_none(),
        "past the place"
    );
    assert!(
        offered((4, 0), (4, 3)).range_around(at).is_none(),
        "before it"
    );
}

#[test]
fn an_item_that_would_insert_a_control_character_is_dropped() {
    let answer = json!([
        {"label": "a", "insertText": "a\u{1b}[2J"},
        {"label": "b", "insertText": "b\rc"},
        {"label": "c", "insertText": "c\n\td"},
    ]);
    let completed = completed(&answer);
    let labels: Vec<_> = completed
        .items
        .iter()
        .map(|item| item.label.as_str())
        .collect();
    assert_eq!(labels, ["c"]);
}

#[test]
fn each_field_is_kept_to_its_bound() {
    let sort = "s".repeat(SORT_TEXT_BYTES_MAX + 10);
    let detail = "d".repeat(DETAIL_BYTES_MAX + 10);
    let filter = "f".repeat(LABEL_BYTES_MAX + 10);
    let text = "t".repeat(INSERT_TEXT_BYTES_MAX);
    let answer = json!([{"label": "x", "sortText": sort, "detail": detail, "filterText": filter, "insertText": text}]);
    let item = &completed(&answer).items[0];
    assert_eq!(item.detail.len(), DETAIL_BYTES_MAX);
    assert_eq!(item.filter.as_ref().map(String::len), Some(LABEL_BYTES_MAX));
    assert_eq!(item.text.len(), INSERT_TEXT_BYTES_MAX, "kept at its bound");
}

fn any_json() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::from),
        any::<i64>().prop_map(Value::from),
        ".{0,12}".prop_map(Value::from),
    ];
    leaf.prop_recursive(4, 64, 6, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..6).prop_map(Value::from),
            prop::collection::hash_map(
                prop_oneof![
                    Just("items".to_owned()),
                    Just("isIncomplete".to_owned()),
                    Just("label".to_owned()),
                    Just("kind".to_owned()),
                    Just("detail".to_owned()),
                    Just("filterText".to_owned()),
                    Just("sortText".to_owned()),
                    Just("textEdit".to_owned()),
                    Just("insertText".to_owned()),
                    Just("insertTextFormat".to_owned()),
                    Just("range".to_owned()),
                    Just("insert".to_owned()),
                    Just("newText".to_owned()),
                    Just("start".to_owned()),
                    Just("line".to_owned()),
                    Just("character".to_owned()),
                ],
                inner,
                0..6,
            )
            .prop_map(|map| Value::Object(map.into_iter().collect())),
        ]
    })
}

proptest! {
    /// Any JSON reads within the bounds and without a panic, and any text as a snippet.
    #[test]
    fn fuzz(result in any_json(), snippet in ".{0,40}") {
        let completed = completed(&result);
        prop_assert!(completed.items.len() <= COMPLETION_ITEMS_MAX);
        for item in &completed.items {
            prop_assert!(item.label.len() <= LABEL_BYTES_MAX);
            prop_assert!(item.detail.len() <= DETAIL_BYTES_MAX);
            prop_assert!(item.text.len() <= INSERT_TEXT_BYTES_MAX);
        }
        let plain = plain(&snippet);
        prop_assert!(plain.len() <= snippet.len());
    }
}

#[test]
fn the_server_s_trigger_characters_and_resolve_are_read() {
    let result = json!({"capabilities": {"completionProvider": {
        "triggerCharacters": [".", ":", "::", " ", "\u{7}"],
        "resolveProvider": true,
    }}});
    let read = completing(&result).unwrap();
    assert_eq!(read.triggers, ['.', ':'], "one printable character each");
    assert!(read.resolves);
    assert_eq!(completing(&json!({"capabilities": {}})), None);
}

#[test]
fn extra_edits_are_read_within_their_bounds_or_not_at_all() {
    let edit = |line: u32, text: &str| json!({"range": {"start": {"line": line, "character": 0}, "end": {"line": line, "character": 0}}, "newText": text});
    let answer = json!([{"label": "x", "additionalTextEdits": [edit(0, "use a::x;\n")]}]);
    let item = &completed(&answer).items[0];
    assert_eq!(
        item.extra,
        [(
            Position::new(0, 0),
            Position::new(0, 0),
            "use a::x;\n".to_owned()
        )]
    );
    let many: Vec<_> = (0..=ADDITIONAL_EDITS_MAX as u32)
        .map(|line| edit(line, "a"))
        .collect();
    assert!(
        extra_edits(Some(&json!(many))).is_empty(),
        "past the most edits"
    );
    let big = "a".repeat(ADDITIONAL_EDITS_BYTES_MAX + 1);
    assert!(
        extra_edits(Some(&json!([edit(0, &big)]))).is_empty(),
        "past the most bytes"
    );
    assert!(
        extra_edits(Some(&json!([edit(0, "a\u{1b}")]))).is_empty(),
        "a control character"
    );
    let resolved_item = json!({"label": "x", "additionalTextEdits": [edit(2, "b")]});
    assert_eq!(resolved(&resolved_item).len(), 1);
}
