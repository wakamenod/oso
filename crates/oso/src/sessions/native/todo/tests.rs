//! A native session's to-do list (ADR 0165, part 3e): the tools' results folded into it, the
//! main thread only, its limits and the turn's end, and the strip's text, folded and opened.
//!
//! Must not depend on: the app; the tests give the stream's lines.

use serde_json::{Value, json};

use super::*;
use crate::sessions::native::wire::parse;

/// The `assistant` line of a call of `name` with `input`, by `parent` if any.
fn call(id: &str, name: &str, input: &Value, parent: Option<&str>) -> Event {
    let line = json!({
        "type": "assistant",
        "parent_tool_use_id": parent,
        "message": {
            "id": "msg",
            "model": "m",
            "content": [{
                "type": "tool_use",
                "id": id,
                "name": name,
                "input": input
            }]
        }
    });
    parse(line.to_string().as_bytes())
}

/// The `user` line of call `id`'s result, which gave back `result`.
fn result(id: &str, result: &Value, error: bool) -> Event {
    let line = json!({
        "type": "user",
        "parent_tool_use_id": null,
        "message": {
            "role": "user",
            "content": [{
                "type": "tool_result",
                "tool_use_id": id,
                "is_error": error,
                "content": "ok"
            }]
        },
        "tool_use_result": result
    });
    parse(line.to_string().as_bytes())
}

fn turn_end() -> Event {
    parse(br#"{"type":"result","subtype":"success","session_id":"s"}"#)
}

/// `todo` after a `TaskCreate` of `subject`, as task `n`.
fn create(todo: &mut Todo, n: u32, subject: &str) {
    let id = format!("c{n}");
    todo.apply(&call(&id, "TaskCreate", &json!({"subject": subject}), None));
    let task = json!({"task": {"id": n.to_string(), "subject": subject}});
    assert!(todo.apply(&result(&id, &task, false)));
}

/// `todo` after a `TaskUpdate` of task `n` to `status`.
fn status(todo: &mut Todo, n: u32, status: &str) -> bool {
    let id = format!("u{n}{status}");
    let input = json!({"taskId": n.to_string(), "status": status});
    todo.apply(&call(&id, "TaskUpdate", &input, None));
    let done = json!({
        "success": true,
        "taskId": n.to_string(),
        "updatedFields": ["status"],
        "statusChange": {"from": "pending", "to": status}
    });
    todo.apply(&result(&id, &done, false))
}

fn texts(todo: &Todo) -> Vec<(&str, Status)> {
    (todo.items().iter())
        .map(|item| (item.text.as_str(), item.status))
        .collect()
}

#[test]
fn tasks_are_created_updated_renamed_and_deleted() {
    let mut todo = Todo::default();
    create(&mut todo, 1, "Alpha");
    create(&mut todo, 2, "Beta");
    create(&mut todo, 3, "Gamma");
    assert!(status(&mut todo, 1, "in_progress"));
    todo.apply(&call(
        "r",
        "TaskUpdate",
        &json!({"taskId": "2", "subject": "Beta2"}),
        None,
    ));
    let renamed = json!({"success": true, "taskId": "2", "updatedFields": ["subject"]});
    assert!(todo.apply(&result("r", &renamed, false)));
    assert!(status(&mut todo, 3, "deleted"));
    assert_eq!(
        texts(&todo),
        [("Alpha", Status::InProgress), ("Beta2", Status::Pending)]
    );
}

#[test]
fn a_written_list_replaces_the_list_whole() {
    let mut todo = Todo::default();
    create(&mut todo, 1, "Alpha");
    todo.apply(&call("w", "TodoWrite", &json!({"todos": []}), None));
    let written = json!({"oldTodos": [], "newTodos": [
        {"content": "One", "status": "completed", "activeForm": "x"},
        {"content": "Two", "status": "in_progress", "activeForm": "y"}
    ]});
    assert!(todo.apply(&result("w", &written, false)));
    assert_eq!(
        texts(&todo),
        [("One", Status::Completed), ("Two", Status::InProgress)]
    );
}

#[test]
fn only_the_main_threads_answered_calls_count() {
    let mut todo = Todo::default();
    let task = json!({"task": {"id": "1", "subject": "Alpha"}});
    // A subagent's call.
    todo.apply(&call("a", "TaskCreate", &json!({}), Some("toolu_parent")));
    assert!(!todo.apply(&result("a", &task, false)));
    // An error, a result of no call, and another tool's result shaped as a task's.
    todo.apply(&call("b", "TaskCreate", &json!({}), None));
    assert!(!todo.apply(&result("b", &task, true)));
    assert!(!todo.apply(&result("c", &task, false)));
    todo.apply(&call("d", "Bash", &json!({"command": "x"}), None));
    assert!(!todo.apply(&result("d", &task, false)));
    // An update of another task than its call named.
    create(&mut todo, 1, "Alpha");
    todo.apply(&call("e", "TaskUpdate", &json!({"taskId": "9"}), None));
    let other = json!({"success": true, "taskId": "1", "statusChange": {"to": "completed"}});
    assert!(!todo.apply(&result("e", &other, false)));
    assert_eq!(texts(&todo), [("Alpha", Status::Pending)]);
}

#[test]
fn a_list_all_done_goes_at_the_turns_end() {
    let mut todo = Todo::default();
    create(&mut todo, 1, "Alpha");
    assert!(!todo.apply(&turn_end()));
    assert!(todo.shows());
    status(&mut todo, 1, "completed");
    assert!(todo.shows());
    assert!(todo.apply(&turn_end()));
    assert!(!todo.shows());
}

#[test]
fn the_list_and_its_calls_are_bounded() {
    let mut todo = Todo::default();
    for n in 0..u32::try_from(MAX_ITEMS).unwrap() {
        create(&mut todo, n, "x");
    }
    let id = "over";
    todo.apply(&call(id, "TaskCreate", &json!({}), None));
    let task = json!({"task": {"id": "x", "subject": "y"}});
    assert!(!todo.apply(&result(id, &task, false)));
    assert_eq!(todo.items().len(), MAX_ITEMS);
    for n in 0..MAX_CALLS + 10 {
        todo.apply(&call(&format!("k{n}"), "TaskCreate", &json!({}), None));
    }
    assert_eq!(todo.calls.len(), MAX_CALLS);
}

/// The texts of `strip` in `face`.
fn in_face(strip: &Strip, face: Face) -> Vec<&str> {
    (strip.faces.iter())
        .filter(|(_, its)| *its == face)
        .map(|(range, _)| &strip.text[range.clone()])
        .collect()
}

#[test]
fn the_folded_strip_fences_claudes_text_and_keeps_its_keys() {
    let mut todo = Todo::default();
    create(&mut todo, 1, "Read the handler");
    create(&mut todo, 2, "Run the auth tests");
    create(&mut todo, 3, "Update\u{1b}[31m the\nchangelog");
    status(&mut todo, 1, "completed");
    status(&mut todo, 2, "in_progress");
    let strip = todo.folded(60);
    assert_eq!(cells_of(&strip.text), 60);
    let head = "▸ Tasks (1/3)  [/] Run the auth tests ";
    assert!(strip.text.starts_with(head), "{}", strip.text);
    assert!(strip.text.ends_with("  C-c C-t list"));
    // The mark and the item as one, the keys in a face of their own, the head as plain text.
    assert_eq!(
        in_face(&strip, Face::Waiting),
        ["[/]", "Run the auth tests"]
    );
    assert_eq!(in_face(&strip, Face::Prompt), ["C-c C-t list"]);
    let first = strip.faces.iter().map(|(range, _)| range.start).min();
    assert_eq!(first, Some("▸ Tasks (1/3)  ".len()));
    // Narrow: Claude's text is cut, and oso's keys stay at the edge.
    let strip = todo.folded(45);
    assert_eq!(strip.text, "▸ Tasks (1/3)  [/] Run the aut…  C-c C-t list");
    assert_eq!(in_face(&strip, Face::Prompt), ["…", "C-c C-t list"]);
    // Too narrow for them and 8 cells of it: the keys go, then the item, then the head is cut.
    assert_eq!(
        todo.folded(41).text,
        "▸ Tasks (1/3)  [/] Run the…  C-c C-t list"
    );
    assert_eq!(
        todo.folded(40).text,
        "▸ Tasks (1/3)  [/] Run the auth tests"
    );
    assert_eq!(todo.folded(30).text, "▸ Tasks (1/3)  [/] Run the au…");
    assert_eq!(todo.folded(20).text, "▸ Tasks (1/3)  [/] …");
    assert_eq!(todo.folded(19).text, "▸ Tasks (1/3)");
    assert_eq!(todo.folded(12).text, "▸ Tasks (1/…");
    // The item not started next, escaped.
    status(&mut todo, 2, "completed");
    let strip = todo.folded(80);
    let head = "▸ Tasks (2/3)  [ ] Update [31m the changelog ";
    assert!(strip.text.starts_with(head), "{}", strip.text);
    assert_eq!(in_face(&strip, Face::Target)[0], "[ ]");
    // A short item's keys stay where it is whole.
    let mut short = Todo::default();
    create(&mut short, 1, "go");
    assert_eq!(short.folded(35).text, "▸ Tasks (0/1)  [ ] go  C-c C-t list");
    assert_eq!(short.folded(34).text, "▸ Tasks (0/1)  [ ] go");
    // All done: no item.
    status(&mut short, 1, "completed");
    assert_eq!(short.folded(30).text, "▸ Tasks (1/1)     C-c C-t list");
    assert_eq!(short.folded(20).text, "▸ Tasks (1/1)");
}

#[test]
fn the_opened_strip_shows_six_rows_at_most_around_the_work() {
    let mut todo = Todo::default();
    for n in 1..=3 {
        create(&mut todo, n, &format!("Item {n}"));
    }
    status(&mut todo, 1, "completed");
    status(&mut todo, 2, "in_progress");
    let strip = todo.opened(40);
    assert_eq!(
        strip.text,
        "▾ Tasks (1/3)               C-c C-t fold\n    [x] Item 1\n    [/] Item 2\n    [ ] Item 3"
    );
    assert_eq!(in_face(&strip, Face::Dim), ["[x]", "Item 1"]);
    assert_eq!(in_face(&strip, Face::Waiting), ["[/]", "Item 2"]);
    assert_eq!(in_face(&strip, Face::Target), ["[ ]", "Item 3"]);
    assert_eq!(in_face(&strip, Face::Prompt), ["C-c C-t fold"]);
    // Five items fill it.
    for n in 4..=5 {
        create(&mut todo, n, &format!("Item {n}"));
    }
    let opened = todo.opened(40).text;
    assert_eq!(opened.lines().count(), OPEN_ROWS);
    assert!(!opened.contains("more"), "{opened}");
    // More: the five around the work, and the head says how many more.
    for n in 6..=9 {
        create(&mut todo, n, &format!("Item {n}"));
    }
    for n in 2..=4 {
        status(&mut todo, n, "completed");
    }
    status(&mut todo, 5, "in_progress");
    let strip = todo.opened(40);
    assert_eq!(
        strip.text,
        "▾ Tasks (4/9)  … 4 more     C-c C-t fold\n    [x] Item 4\n    [/] Item 5\n    [ ] Item 6\n    [ ] Item 7\n    [ ] Item 8"
    );
    assert_eq!(in_face(&strip, Face::Prompt), ["… 4 more", "C-c C-t fold"]);
    // Narrow: the keys go, then how many more, then the head is cut; an item's text is cut.
    let first = |width| todo.opened(width).text.lines().next().unwrap().to_owned();
    assert_eq!(first(37), "▾ Tasks (4/9)  … 4 more  C-c C-t fold");
    assert_eq!(first(36), "▾ Tasks (4/9)  … 4 more");
    assert_eq!(first(22), "▾ Tasks (4/9)");
    assert_eq!(todo.opened(12).text.lines().nth(1), Some("    [x] Ite…"));
}

#[test]
fn every_row_is_at_most_the_strips_width() {
    let mut todo = Todo::default();
    for n in 1..=100 {
        create(
            &mut todo,
            n,
            "テストを実行する, all of them, then say what they said",
        );
    }
    status(&mut todo, 1, "in_progress");
    for width in 0..=130 {
        for strip in [todo.folded(width), todo.opened(width)] {
            assert!(fits(&strip, width), "{width}: {:?}", strip.text);
        }
        assert_eq!(
            todo.folded(width).text.lines().count(),
            usize::from(width > 0)
        );
        assert!(todo.opened(width).text.split('\n').count() <= OPEN_ROWS);
    }
}

#[test]
fn a_resumed_session_has_no_list() {
    let mut todo = Todo::default();
    create(&mut todo, 1, "Alpha");
    todo.apply(&call("u", "TaskUpdate", &json!({"taskId": "1"}), None));
    assert!(todo.clear());
    assert!(!todo.clear());
    let done = json!({"success": true, "taskId": "1", "statusChange": {"to": "completed"}});
    assert!(!todo.apply(&result("u", &done, false)));
    assert!(!todo.shows());
}

/// Whether every row of `strip`, measured whole as the display draws it, is at most `width`
/// cells, and each stretch of an item's starts and ends where a cluster of the whole text does.
fn fits(strip: &Strip, width: usize) -> bool {
    use oso_core::display_map::text_clusters;
    let rows = strip.text.split('\n').all(|row| cells_of(row) <= width);
    let mut starts = Vec::new();
    let mut at = 0;
    for row in strip.text.split('\n') {
        starts.extend(text_clusters(row, 0).map(|cluster| at + cluster.offset));
        at += row.len();
        starts.push(at);
        at += 1;
    }
    let fenced = (strip.faces.iter())
        .filter(|(_, face)| matches!(face, Face::Target | Face::Waiting | Face::Dim))
        .all(|(range, _)| starts.contains(&range.start) && starts.contains(&range.end));
    rows && fenced
}

#[test]
fn marks_that_would_join_oso_s_text_go_on_their_own_base() {
    let mut todo = Todo::default();
    create(&mut todo, 1, &format!("{}tail", "\u{301}".repeat(12)));
    create(&mut todo, 2, "ends in a prepend \u{600}");
    create(&mut todo, 3, "\u{200d}\u{fe0f}joined");
    create(&mut todo, 4, &format!("x{}", "\u{600}".repeat(100)));
    status(&mut todo, 1, "in_progress");
    for width in [12, 20, 30, 50, 80] {
        let folded = todo.folded(width);
        assert!(fits(&folded, width), "{width}: {:?}", folded.text);
        let opened = todo.opened(width);
        assert!(fits(&opened, width), "{width}: {:?}", opened.text);
    }
    assert!(todo.folded(80).text.contains("[/] \u{25cc}\u{301}"));
    // A run of prepends past a cluster's limit, cut: oso's `…` stays its own.
    status(&mut todo, 1, "completed");
    status(&mut todo, 4, "in_progress");
    for width in [20, 30, 50, 80] {
        let folded = todo.folded(width);
        assert!(fits(&folded, width), "{width}: {:?}", folded.text);
    }
    for width in 0..3 {
        assert!(
            todo.opened(width)
                .text
                .lines()
                .all(|row| cells_of(row) <= 2)
        );
    }
}

#[test]
fn the_same_list_written_again_changes_nothing() {
    let mut todo = Todo::default();
    let written = json!({"oldTodos": [], "newTodos": [
        {"content": "One", "status": "pending", "activeForm": "x"}
    ]});
    todo.apply(&call("w1", "TodoWrite", &json!({}), None));
    assert!(todo.apply(&result("w1", &written, false)));
    todo.apply(&call("w2", "TodoWrite", &json!({}), None));
    assert!(!todo.apply(&result("w2", &written, false)));
    todo.apply(&call("c", "TaskCreate", &json!({}), None));
    let again = json!({"task": {"id": "0", "subject": "One"}});
    assert!(!todo.apply(&result("c", &again, false)));
}
