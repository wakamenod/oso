//! A call's block (ADR 0182): its text under the title, its change, its bounds and its title.
//!
//! Must not depend on: the app; the tests give what the call says.

use oso_core::minibuffer::cells_of;
use serde_json::json;

use super::body::{BASE, MAX_LINES, NO_BREAK, SPACE};
use super::*;
use crate::sessions::native::render::Face;

/// A deadline no test reaches.
pub(super) fn later() -> Instant {
    Instant::now() + Duration::from_secs(60)
}

pub(super) fn block(tool: &str, input: &Value) -> Block {
    Block::call(tool, Some(input), later())
}

/// The block of a change of `old` into `new`.
fn edit(old: &str, new: &str) -> Block {
    let input = json!({"file_path": "/work/src/a.rs", "old_string": old, "new_string": new});
    block("Edit", &input)
}

/// Its body's lines, without the file's.
fn changed(block: &Block) -> Vec<&str> {
    block.body().text.lines().skip(1).collect()
}

/// The words of `block`'s title at 100 columns.
fn title(block: &Block) -> String {
    let title = block.title((1, 1), 100).text;
    title.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The face of the text `part` of `body`'s, where it is first.
fn face_of(body: &Rendered, part: &str) -> Option<Face> {
    let at = body.text.find(part).unwrap();
    (body.faces.iter())
        .find(|(range, _)| range.contains(&at))
        .map(|(_, face)| *face)
}

#[test]
fn a_command_is_whole_with_why_and_every_other_field() {
    let block = block(
        "Bash",
        &json!({
            "command": "cargo test \\\n  -p auth\n",
            "description": "Run the tests\nof auth",
            "timeout": 600_000,
            "env": {"A": ["b"]},
            "note": "one\ntwo",
            "empty": "",
        }),
    );
    let expected = "$ cargo test \\\n    -p auth\n\nWhy\n  Run the tests\n  of auth\nIts other fields\n  \
                    empty \"\"\n  env\n    {\n      \"A\": [\n        \"b\"\n      ]\n    }\n  note\n    \
                    one\n    two\n  timeout 600000\n";
    assert_eq!(block.body().text, expected);
    assert_eq!(block.hidden(), Hidden::Nothing);
    assert_eq!(title(&block), "⚠ Bash asks to run");
    let body = block.body();
    assert_eq!(face_of(body, "$ "), Some(Face::RequestBody));
    assert_eq!(face_of(body, "cargo"), None);
    assert_eq!(face_of(body, "Run"), Some(Face::Dim));
    assert_eq!(face_of(body, "timeout"), None);
    // An empty line is on the tint too, as each is.
    assert_eq!(face_of(body, "\n\n"), Some(Face::RequestBody));
}

#[test]
fn a_change_is_a_diff_of_all_of_it() {
    let block = edit("a\nb\nc\nd", "a\nB\nc\nd\ne");
    let expected = ["  a", "- b", "+ B", "  c", "  d", "+ e"];
    assert_eq!(changed(&block), expected);
    assert_eq!(block.body().text.lines().next(), Some("  /work/src/a.rs"));
    assert_eq!(
        title(&block),
        "⚠ Edit asks to change a file adds 2 lines, removes 1"
    );
    let body = block.body();
    assert_eq!(face_of(body, "+ B"), Some(Face::Added));
    assert_eq!(face_of(body, "- b"), Some(Face::Removed));
    assert_eq!(face_of(body, "a\n- "), Some(Face::Dim));
    // Past its deadline, all of the old text, then all of the new.
    let input = json!({"file_path": "a", "old_string": "a\nb\n", "new_string": "a\nc\n"});
    let late = Block::call("Edit", Some(&input), Instant::now());
    assert_eq!(changed(&late), ["- a", "- b", "+ a", "+ c"]);
    assert_eq!(
        title(&late),
        "⚠ Edit asks to change a file adds 2 lines, removes 2"
    );
    // Nothing changed.
    let same = edit("a\n", "a\n");
    assert_eq!(changed(&same), ["  a"]);
    assert_eq!(
        title(&same),
        "⚠ Edit asks to change a file adds 0 lines, removes 0"
    );
}

#[test]
fn a_change_of_white_space_or_of_a_line_break_alone_shows() {
    assert_eq!(changed(&edit("a b\n", "a  b\n")), ["- a b", "+ a  b"]);
    assert_eq!(changed(&edit("\ta\n", "  a\n")), ["- \ta", "+   a"]);
    assert_eq!(changed(&edit("a \n", "a\n")), ["- a ", SPACE, "+ a"]);
    // The space keeps the return from being read as a part of the line break.
    let returned = edit("a\r\nb\n", "a\nb\n").body().text.clone();
    assert_eq!(returned, "  /work/src/a.rs\n- a\r \n+ a\n  b\n");
    assert_eq!(
        changed(&edit("a\nb", "a\nb\n")),
        ["  a", "- b", NO_BREAK, "+ b"]
    );
    assert_eq!(
        changed(&edit("a\nb\n", "a\nb")),
        ["  a", "- b", "+ b", NO_BREAK]
    );
    // Pieces of lines both, as most changes are.
    assert_eq!(changed(&edit("b", "c")), ["- b", "+ c"]);
    assert_eq!(changed(&edit("b", "")), ["- b", NO_BREAK]);
    assert_eq!(changed(&edit("b\n", "")), ["- b"]);
    assert_eq!(changed(&edit("", "b")), ["+ b", NO_BREAK]);
    assert_eq!(changed(&edit("a\n\n", "a\n")), ["  a", "-"]);
    assert_eq!(
        face_of(edit("a\nb", "a\nb\n").body(), NO_BREAK),
        Some(Face::RequestBody)
    );
}

#[test]
fn a_file_written_whole_and_several_changes() {
    let write = block(
        "Write",
        &json!({"file_path": "/etc/hosts", "content": "a\nb"}),
    );
    assert_eq!(
        write.body().text,
        format!("  /etc/hosts\n+ a\n+ b\n{NO_BREAK}\n")
    );
    assert_eq!(
        title(&write),
        "⚠ Write asks to change a file writes 2 lines"
    );
    let several = block(
        "MultiEdit",
        &json!({"file_path": "/work/a", "edits": [
            {"old_string": "a\n", "new_string": "b\n", "replace_all": true},
            {"old_string": "c\n", "new_string": ""},
        ]}),
    );
    let expected =
        "  /work/a\nChange 1 of 2\n  replace_all true\nThe change\n- a\n+ b\nChange 2 of 2\n- c\n";
    assert_eq!(several.body().text, expected);
    assert_eq!(
        title(&several),
        "⚠ MultiEdit asks to change a file adds 1 line, removes 2"
    );
    let other = block("mcp__x__y", &json!({"description": "z", "b": null}));
    assert_eq!(
        other.body().text,
        "  mcp__x__y\n  b null\n  description z\n"
    );
    let none = block("mcp__x__y", &json!({}));
    assert_eq!(none.body().text, "  mcp__x__y\nIt has no input\n");
}

#[test]
fn a_mark_that_would_join_osos_goes_on_a_base() {
    for mark in ["\u{301}", "\u{903}", "\u{20e3}", "\u{1f3fb}", "\u{336}"] {
        let block = block("Bash", &json!({"command": format!("{mark}a\n{mark}b")}));
        let expected = format!("$ {BASE}{mark}a\n  {BASE}{mark}b\n");
        assert_eq!(block.body().text, expected, "{mark:?}");
    }
    // What the display escapes by itself, and what joins nothing, stay as they are.
    for other in [
        "\u{200d}",
        "\u{202e}",
        "\u{1b}",
        "\u{2028}",
        "\u{3000}a",
        "あ",
        "\u{1f1ef}",
    ] {
        let block = block("Bash", &json!({"command": other}));
        assert_eq!(block.body().text, format!("$ {other}\n"), "{other:?}");
    }
    // A field's name is on its line.
    let block = block("X", &json!({"a\n- b": 1, "timeout 2": 3, "": 4}));
    let expected = "  X\n  \"\" 4\n  \"a\\n- b\" 1\n  \"timeout 2\" 3\n";
    assert_eq!(block.body().text, expected);
}

#[test]
fn what_is_past_the_bounds_is_counted() {
    let all = block("Bash", &json!({"command": "a\n".repeat(MAX_LINES - 1)}));
    assert_eq!(all.hidden(), Hidden::Nothing);
    assert_eq!(all.body().text.lines().count(), MAX_LINES);
    let more = block(
        "Bash",
        &json!({"command": "a\n".repeat(MAX_LINES + 1), "x": "a\nb"}),
    );
    assert_eq!(more.hidden(), Hidden::Lines(6));
    let text = &more.body().text;
    assert_eq!(text.lines().count(), MAX_LINES + 1);
    assert!(
        text.ends_with("\n… 6 lines more not shown\n"),
        "{}",
        &text[text.len() - 40..]
    );
    let old = "a\n".repeat(MAX_LINES);
    let change = edit(&old, "");
    assert_eq!(change.hidden(), Hidden::Lines(1));
    assert_eq!(
        title(&change),
        "⚠ Edit asks to change a file adds 0 lines, removes 2000"
    );
    for none in [
        Block::call("Bash", None, later()),
        block("Bash", &json!({"command": "x".repeat(64 << 10)})),
    ] {
        assert_eq!(none.hidden(), Hidden::All);
        assert_eq!(
            none.body().text,
            "Its input is over 64 KiB, or cannot be read: not shown at all\n"
        );
    }
}

#[test]
fn a_title_is_one_row_at_any_width_and_names_the_tool_alone() {
    let edit = edit("a\n", "b\n");
    let at = |place, width| edit.title(place, width).text;
    let counted = "1 of 2 · adds 1 line, removes 1";
    let gap = " ".repeat(80 - 28 - counted.chars().count());
    assert_eq!(
        at((1, 2), 80),
        format!("⚠ Edit asks to change a file{gap}{counted}\n")
    );
    assert_eq!(
        at((1, 2), 40),
        format!("⚠ Edit asks to change a file{}1 of 2\n", " ".repeat(6))
    );
    assert_eq!(at((1, 1), 40), "⚠ Edit asks to change a file\n");
    assert_eq!(at((1, 1), 20), "⚠ Edit asks to chan…\n");
    let long = block(&"mcp__a_server__".repeat(9), &json!({}));
    for width in 0..140 {
        for shown in [&edit, &long] {
            let title = shown.title((1, 3), width);
            let row = title.text.strip_suffix('\n').unwrap();
            assert!(
                !row.contains('\n') && cells_of(row) <= width.max(1),
                "{width}: {row:?}"
            );
            assert!(title.faces.iter().all(|(_, face)| *face == Face::CardTitle));
        }
    }
    // What a call asks stays in sight.
    assert_eq!(long.title((1, 1), 24).text, "⚠ mcp__a_se… asks to run\n");
    assert!(long.title((1, 1), 100).text.contains("… asks to run"));
    let odd = block("Ba\u{301}sh asks\n⚠", &json!({}));
    assert_eq!(
        title(&odd),
        "⚠ Ba\\u{301}sh\\u{20}asks\\u{a}\\u{26a0} asks to run"
    );
    assert_eq!(title(&block("", &json!({}))), "⚠ A tool asks to run");
}

#[test]
fn osos_words_stand_between_parts_that_could_be_read_on() {
    let text = |tool: &str, input: &Value| block(tool, input).body().text.clone();
    let input = json!({"command": "a", "description": "b", "timeout": 1});
    assert_eq!(text("Bash", &input), "$ a\n  b\n  timeout 1\n");
    let input = json!({"command": "a", "description": "timeout 1"});
    assert_eq!(text("Bash", &input), "$ a\nWhy\n  timeout 1\n");
    let long = "b".repeat(41);
    let input = json!({"a": long});
    assert_eq!(text("X", &input), format!("  X\n  a\n    {long}\n"));
    let input = json!({"file_path": "\"a\""});
    assert_eq!(text("Write", &input), "  \"\\\"a\\\"\"\n");
    let input = json!({"command": "a\nb", "description": "b", "timeout": 1});
    assert_eq!(text("Bash", &input), "$ a\n  b\nWhy\n  b\n  timeout 1\n");
    let input = json!({
        "file_path": "a\nb", "old_string": "c", "new_string": "c", "replace_all": true, "x": 1,
    });
    let expected = "  \"a\\nb\"\n  replace_all true\nThe change\n  c\nIts other fields\n  x 1\n";
    assert_eq!(text("Edit", &input), expected);
    let input = json!({"file_path": "a", "edits": [{"x": 1}, {"y": 1}], "z": 1});
    let expected =
        "  a\nChange 1 of 2\n  x 1\nChange 2 of 2\n  y 1\nThe call's other fields\n  z 1\n";
    assert_eq!(text("MultiEdit", &input), expected);
    let input = json!({"file_path": "a", "content": ""});
    assert_eq!(text("Write", &input), "  a\nNothing is written\n");
    let input = json!({"file_path": "a", "old_string": "", "new_string": ""});
    assert_eq!(text("Edit", &input), "  a\nNothing is changed\n");
}

#[test]
fn white_space_at_a_lines_end_is_said() {
    for blank in [" ", "\t", "\u{a0}", "\u{3000}", "\u{2800}"] {
        let block = block("Bash", &json!({"command": format!("a \\{blank}\nb")}));
        assert_eq!(block.body().text, format!("$ a \\{blank}\n{SPACE}\n  b\n"));
    }
    assert_eq!(
        changed(&edit("a \nb", "a \nc ")),
        ["  a ", "- b", "+ c ", SPACE]
    );
    let block = block("X", &json!({"a": "b \nc"}));
    assert_eq!(
        block.body().text,
        format!("  X\n  a\n    b \n{SPACE}\n    c\n")
    );
}

#[test]
fn a_tools_whole_name_is_in_its_block() {
    let long = "mcp__plugin_customer-support_google-calendar__delete_";
    let of = |name: &str| block(name, &json!({}));
    let (event, events) = (of(&format!("{long}event")), of(&format!("{long}entry")));
    assert_eq!(event.title((1, 1), 60), events.title((1, 1), 60));
    assert_eq!(
        event.body().text,
        format!("  {long}event\nIt has no input\n")
    );
    assert_ne!(event.body(), events.body());
    // One over the bounds says it too.
    let none = Block::call(&format!("{long}é"), None, later());
    assert!(
        none.body()
            .text
            .starts_with(&format!("  {long}\\u{{e9}}\nIts input"))
    );
}
