//! A call's parts as a block shows them (ADR 0182): their order, and every field once.
//!
//! Must not depend on: the app; the tests give a call's input.

use serde_json::json;

use super::*;

fn text(text: &str) -> Shown {
    Shown::Text(text.to_owned())
}

fn json(text: &str) -> Shown {
    Shown::Json(text.to_owned())
}

fn field(name: &str, value: Shown) -> Part {
    Part::Field(name.to_owned(), value)
}

#[test]
fn what_runs_is_first_then_why_then_every_other_field() {
    let input = json!({
        "timeout": 600_000,
        "description": "Run the tests",
        "command": "cargo test",
        "dangerouslyDisableSandbox": true,
    });
    let expected = [
        Part::Command("cargo test".into()),
        Part::Why("Run the tests".into()),
        field("dangerouslyDisableSandbox", json("true")),
        field("timeout", json("600000")),
    ];
    assert_eq!(parts("Bash", &input).unwrap(), expected);
}

#[test]
fn a_known_field_of_another_type_is_shown_as_any_field() {
    let input = json!({"command": ["rm"], "description": ""});
    let expected = [
        field("command", json("[\n  \"rm\"\n]")),
        field("description", json("\"\"")),
    ];
    assert_eq!(parts("Bash", &input).unwrap(), expected);
    let input = json!({"file_path": 1, "old_string": "a", "new_string": null});
    let expected = [
        field("file_path", json("1")),
        field("new_string", json("null")),
        field("old_string", text("a")),
    ];
    assert_eq!(parts("Edit", &input).unwrap(), expected);
    let input = json!({"file_path": "/a", "edits": [1]});
    let expected = [Part::File("/a".into()), field("edits", json("[\n  1\n]"))];
    assert_eq!(parts("MultiEdit", &input).unwrap(), expected);
    assert_eq!(
        parts("MultiEdit", &json!({"edits": []})).unwrap(),
        [field("edits", json("[]"))]
    );
    assert_eq!(parts("Bash", &json!("ls")), None);
    assert_eq!(parts("Bash", &json!({})).unwrap(), []);
}

#[test]
fn a_change_is_under_its_file_and_whether_it_is_made_everywhere() {
    let input = json!({
        "file_path": "/w/a", "old_string": "a", "new_string": "b", "replace_all": true, "x": 1,
    });
    let change = || Part::Change {
        old: "a".into(),
        new: "b".into(),
    };
    let everywhere = || field("replace_all", json("true"));
    let expected = [
        Part::File("/w/a".into()),
        everywhere(),
        change(),
        field("x", json("1")),
    ];
    assert_eq!(parts("Edit", &input).unwrap(), expected);
    let input = json!({
        "file_path": "/w/a",
        "edits": [
            {"old_string": "a", "new_string": "b", "replace_all": true, "x": "y"},
            {"old_string": "a", "new_string": "b"},
            {"old_string": "a"},
        ],
    });
    let expected = [
        Part::File("/w/a".into()),
        Part::Nth(1, 3),
        everywhere(),
        change(),
        field("x", text("y")),
        Part::Nth(2, 3),
        change(),
        Part::Nth(3, 3),
        field("old_string", text("a")),
    ];
    assert_eq!(parts("MultiEdit", &input).unwrap(), expected);
    // The call's own fields are told from its last change's, and an edit that says nothing is said.
    let input = json!({"edits": [{"a": 1}, {"b": 2}], "c": 3});
    let expected = [
        Part::Nth(1, 2),
        field("a", json("1")),
        Part::Nth(2, 2),
        field("b", json("2")),
        Part::Call,
        field("c", json("3")),
    ];
    assert_eq!(parts("MultiEdit", &input).unwrap(), expected);
    let input = json!({"edits": [{"a": 1}], "c": 3});
    let expected = [field("a", json("1")), Part::Call, field("c", json("3"))];
    assert_eq!(parts("MultiEdit", &input).unwrap(), expected);
    let input = json!({"edits": [{}]});
    let expected = [field("edits", json("[\n  {}\n]"))];
    assert_eq!(parts("MultiEdit", &input).unwrap(), expected);
    let input = json!({"file_path": "/w/a", "content": "a\n", "edits": []});
    let expected = [
        Part::File("/w/a".into()),
        Part::Written("a\n".into()),
        field("edits", json("[]")),
    ];
    assert_eq!(parts("Write", &input).unwrap(), expected);
}

#[test]
fn an_input_is_read_whole_or_not_at_all() {
    let whole = json!({"command": "x".repeat(MAX_INPUT - r#"{"command":""}"#.len())});
    let shown = parts("Bash", &whole).unwrap();
    assert!(matches!(&shown[..], [Part::Command(command)] if command.len() == MAX_INPUT - 14));
    let long = json!({"command": "x".repeat(MAX_INPUT)});
    assert_eq!(parts("Bash", &long), None);
    // One whose indented text would end in a character is not shown up to it.
    let wide = json!({"x": [[[["あ".repeat(21_836), "rm"]]]]});
    assert!(fits(&wide, MAX_INPUT));
    let shown = parts("X", &wide).unwrap();
    assert_eq!(shown, [field("x", json(&wide["x"].to_string()))]);
    // What nests is indented, unless that takes more than an input does, all together.
    let mut deep = json!(1);
    for _ in 0..100 {
        deep = json!([deep]);
    }
    let many = json!({"x": vec![deep; 300]});
    let shown = parts("X", &many).unwrap();
    let [Part::Field(_, Shown::Json(one))] = &shown[..] else {
        panic!("one field");
    };
    assert!(!one.contains('\n') && one.len() <= MAX_INPUT);
    let mut deep = json!(1);
    for _ in 0..100 {
        deep = json!([deep]);
    }
    let many: Map<String, Value> = (0..40).map(|at| (at.to_string(), deep.clone())).collect();
    let shown = parts("X", &Value::Object(many)).unwrap();
    let bytes = shown.iter().map(|part| match part {
        Part::Field(_, Shown::Json(text)) => text.len(),
        _ => 0,
    });
    let bytes: usize = bytes.sum();
    assert!((MAX_INPUT / 2..2 * MAX_INPUT).contains(&bytes), "{bytes}");
}

#[test]
fn a_text_that_reads_as_another_value_is_json() {
    let input = json!({
        "a": "true", "b": true, "c": "", "d": "\"\"", "e": " x", "f": "x\n ", "g": "{\n}", "h": "a b",
    });
    let expected = [
        field("a", json("\"true\"")),
        field("b", json("true")),
        field("c", json("\"\"")),
        field("d", json("\"\\\"\\\"\"")),
        field("e", json("\" x\"")),
        field("f", text("x\n ")),
        field("g", json("\"{\\n}\"")),
        field("h", text("a b")),
    ];
    assert_eq!(parts("X", &input).unwrap(), expected);
}
