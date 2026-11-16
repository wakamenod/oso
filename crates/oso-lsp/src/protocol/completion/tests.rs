//! Tests of reading a completion answer (ADR 0138, step 9): its two shapes, the order by
//! `sortText`, the bounds, the edit's range of either kind, and snippets as plain text.
//!
//! Must not depend on: `server`, threads or processes.

use serde_json::json;

use super::*;

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
    assert_eq!(completed_of(json!(null)), Completed::default());
}

fn completed_of(value: serde_json::Value) -> Completed {
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
    let params = completion_params("file:///a.rs", Position::new(2, 5));
    assert_eq!(params["context"]["triggerKind"], 1);
    assert_eq!(params["position"]["character"], 5);
    assert_eq!(kind_name(22), "struct");
    assert_eq!(kind_name(99), "");
}
