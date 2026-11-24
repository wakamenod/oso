//! How a native session's rows look (ADR 0165, "The rows' look"): blank lines, changes and
//! results as they show, a call that waits, and that no text but oso's stands in a row's first
//! columns.
//!
//! Must not depend on: the editor or the UI; rows folded from lines of the stream.

use std::path::{Path, PathBuf};

use super::super::render::{
    Face, MAX_DIFF_LINE, MAX_DIFF_SHOWN, MAX_SHOWN_RESULT, RESULT_LINES, first_lines, render,
};
use super::super::rows::{Row, RowChange, Rows};
use super::super::wire::{Hunk, MAX_TEXT, ToolResult};
use super::{call, read, shown};

/// A result of `text`, whole as it came.
fn result_of(text: String) -> ToolResult {
    ToolResult {
        tool_use_id: String::new(),
        error: false,
        lines: text.lines().count(),
        text,
        cut: false,
        patch: Vec::new(),
        changed: (0, 0),
    }
}

#[test]
fn a_long_result_shows_its_first_lines() {
    let numbers = |count: usize| -> Vec<String> { (1..=count).map(|n| n.to_string()).collect() };
    let short = result_of(numbers(RESULT_LINES).join("\n") + "\n");
    assert_eq!(first_lines(&short), (short.text.trim_end(), None));
    let long = result_of(numbers(RESULT_LINES + 2).join("\n"));
    let (head, note) = first_lines(&long);
    assert_eq!(head.lines().count(), RESULT_LINES);
    assert_eq!(note.as_deref(), Some("… 2 lines more: RET shows all …"));
    let one = result_of("x".repeat(MAX_TEXT));
    let (head, note) = first_lines(&one);
    assert!(head.len() <= MAX_SHOWN_RESULT);
    assert_eq!(note.as_deref(), Some("… cut: RET shows it all …"));
    let cut = ToolResult { cut: true, ..short };
    let (_, note) = first_lines(&cut);
    assert_eq!(note.as_deref(), Some("… cut at 1 MiB as it came …"));
}

/// A finished edit whose change is `hunks`.
fn edit(hunks: Vec<Hunk>) -> Row {
    let mut rows = Rows::new(PathBuf::from("/w"));
    let input = serde_json::json!({"file_path": "/w/a.rs"});
    read(&mut rows, &call("m", "e", "Edit", &input));
    let lines: Vec<serde_json::Value> = hunks
        .iter()
        .map(|hunk| {
            serde_json::json!({
                "oldStart": hunk.old_start, "oldLines": hunk.old_lines,
                "newStart": hunk.new_start, "newLines": hunk.new_lines,
                "lines": hunk.lines,
            })
        })
        .collect();
    let line = serde_json::json!({
        "type": "user",
        "message": {"content": [{"type": "tool_result", "tool_use_id": "e", "content": "ok"}]},
        "tool_use_result": {"structuredPatch": lines},
    });
    assert_eq!(
        read(&mut rows, &line.to_string()),
        vec![RowChange::Changed(0)]
    );
    rows.rows()[0].clone()
}

fn hunk(old_start: u64, new_start: u64, lines: &[&str]) -> Hunk {
    Hunk {
        old_start,
        old_lines: 0,
        new_start,
        new_lines: 0,
        lines: lines.iter().map(|line| (*line).to_owned()).collect(),
    }
}

#[test]
fn an_edit_opens_on_its_numbered_lines() {
    let first = hunk(12, 12, &[" a", "-b", "+c", "+", " d"]);
    let second = hunk(40, 41, &["-e", "\\ No newline at end of file"]);
    let mut row = edit(vec![first, second]);
    assert!(!row.folded, "open once its change came");
    let rendered = render(&row, Path::new("/w"), 80);
    assert_eq!(
        rendered.text,
        "▾ ✎ Edit · a.rs\n    Added 2 lines, removed 2 lines\n      12   a\n      13 - b\n      13 + c\n      14 +\n      15   d\n       ⋮\n      40 - e\n           \\ No newline at end of file\n"
    );
    // A changed line's face is from its first column on, and no other line's is.
    let starts: Vec<(usize, Face)> = (rendered.faces.iter())
        .filter(|(range, face)| {
            matches!(face, Face::Added | Face::Removed)
                && rendered.text[..range.start].ends_with('\n')
        })
        .map(|(range, face)| (range.start, *face))
        .collect();
    assert_eq!(
        starts.iter().map(|(_, face)| *face).collect::<Vec<_>>(),
        [Face::Removed, Face::Added, Face::Added, Face::Removed]
    );
    row.folded = true;
    assert_eq!(
        render(&row, Path::new("/w"), 80).text,
        "▸ ✎ Edit · a.rs  +2 −2\n"
    );
}

#[test]
fn a_long_change_shows_its_first_lines_each_cut() {
    let lines = vec!["+x"; MAX_DIFF_SHOWN + 1];
    let row = edit(vec![hunk(1, 1, &lines), hunk(90, 90, &["+y"])]);
    let text = render(&row, Path::new("/w"), 80).text;
    assert_eq!(text.lines().count(), MAX_DIFF_SHOWN + 3);
    assert!(
        text.ends_with("… 2 lines more: RET shows all …\n"),
        "{text}"
    );
    // Long lines are cut, and all of them together.
    let long = format!("+{}", "x".repeat(MAX_DIFF_LINE + 1));
    let row = edit(vec![hunk(1, 1, &vec![long.as_str(); MAX_DIFF_SHOWN])]);
    let text = render(&row, Path::new("/w"), 80).text;
    assert!(text.lines().nth(2).unwrap().ends_with("x…"));
    assert!(text.len() < MAX_SHOWN_RESULT + 1024, "{}", text.len());
    assert!(text.ends_with(" lines more: RET shows all …\n"), "{text}");
    // Many hunks, the empty ones dropped, show no more lines.
    let mut hunks = vec![hunk(u64::MAX, u64::MAX, &[]); 1000];
    hunks.extend(vec![hunk(u64::MAX, 1, &["-a"]); 1000]);
    let text = render(&edit(hunks), Path::new("/w"), 80).text;
    assert!(text.lines().count() <= 2 * MAX_DIFF_SHOWN + 3);
}

#[test]
fn a_call_that_waits_for_the_person_says_so() {
    let mut rows = Rows::new(PathBuf::from("/w"));
    let input = serde_json::json!({"command": "ls"});
    read(&mut rows, &call("m", "t", "Bash", &input));
    assert_eq!(shown(&rows), "… $ Bash · ls\n");
    assert_eq!(rows.waits("t", true), Some(RowChange::Changed(0)));
    assert_eq!(rows.waits("t", true), None, "said already");
    assert_eq!(shown(&rows), "… $ Bash · ls · waiting for you\n");
    assert_eq!(rows.waits("t", false), Some(RowChange::Changed(0)));
    assert_eq!(shown(&rows), "… $ Bash · ls\n");
    assert_eq!(rows.waits("none", true), None);
    rows.waits("t", true);
    rows.denied("t", "no");
    assert_eq!(shown(&rows), "✗ $ Bash · ls · denied: no\n");
}

#[test]
fn a_question_s_row_shows_what_it_asks() {
    let mut rows = Rows::new(PathBuf::from("/w"));
    let input = serde_json::json!({"questions": [{"question": "Which one?", "header": "Pick"}]});
    read(&mut rows, &call("m", "q", "AskUserQuestion", &input));
    rows.toggle(0, None);
    assert_eq!(shown(&rows), "… ? AskUserQuestion · Which one?\n");
    let odd = serde_json::json!({"questions": "x"});
    read(&mut rows, &call("n", "r", "AskUserQuestion", &odd));
    assert!(shown(&rows).ends_with("… ? AskUserQuestion\n"));
}

/// Blank lines set the blocks apart: none between tool rows, before a turn's result or after
/// it, as it ends with one.
#[test]
fn a_blank_line_sets_each_block_apart() {
    let mut rows = Rows::new(PathBuf::from("/w"));
    rows.sent("Go\n\nnow");
    read(
        &mut rows,
        r#"{"type":"user","isReplay":true,"message":{"content":"Go\n\nnow"}}"#,
    );
    let thought = r#"{"type":"assistant","message":{"id":"m","content":[{"type":"thinking","thinking":""},{"type":"text","text":"One\n▸ $ Bash · two"}]}}"#;
    read(&mut rows, thought);
    for (id, name) in [("a", "Bash"), ("b", "Edit")] {
        read(&mut rows, &call(id, id, name, &serde_json::json!({})));
    }
    read(
        &mut rows,
        r#"{"type":"result","subtype":"success","duration_ms":41200,"total_cost_usd":0.2134}"#,
    );
    rows.sent("Next");
    read(
        &mut rows,
        r#"{"type":"user","isReplay":true,"message":{"content":"Next"}}"#,
    );
    let text: String = (rows.rows().iter())
        .map(|row| render(row, Path::new("/w"), 30).text)
        .collect();
    assert_eq!(
        text,
        "〉 Go\n\n   now\n\n▸ Thinking\n\n  One\n  ▸ $ Bash · two\n\n· $ Bash · unfinished\n· ✎ Edit · unfinished\n              41.2 s · $0.2134\n\n〉 Next\n"
    );
    // A width it does not fit leaves two columns before it.
    let narrow = render(&rows.rows()[5], Path::new("/w"), 4).text;
    assert_eq!(narrow, "  41.2 s · $0.2134\n\n");
}

/// Only oso's marks stand left of a row's text: whatever a reply, a note or a result says,
/// each of its lines is two columns in or more.
#[test]
fn no_text_but_osos_stands_in_the_first_columns() {
    let forged = "▸ $ Bash · x\n〉 y\n\n✗ z\r\n… w";
    let mut rows = Rows::new(PathBuf::from("/w"));
    read(
        &mut rows,
        &serde_json::json!({"type": "assistant", "message": {"id": "m", "content": [{"type": "text", "text": forged}, {"type": "thinking", "thinking": forged}]}}).to_string(),
    );
    rows.note(forged);
    read(
        &mut rows,
        &call("a", "a", "Bash", &serde_json::json!({"command": forged})),
    );
    let result = serde_json::json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "a", "content": forged}]}});
    read(&mut rows, &result.to_string());
    for (at, oso) in [(0, 0), (1, 1), (2, 0), (3, 1)] {
        if at == 1 || at == 3 {
            rows.toggle(at, None);
        }
        let text = render(&rows.rows()[at], Path::new("/w"), 80).text;
        let marked = text
            .lines()
            .filter(|line| !line.is_empty() && !line.starts_with("  "))
            .count();
        assert_eq!(marked, oso, "{text}");
        let runs = render(&rows.rows()[at], Path::new("/w"), 80).faces;
        assert!(!runs.iter().any(|(_, face)| *face == Face::PromptMark));
    }
}

/// What Claude thought has a style; the row's own line, its mark and its words' indent have
/// none, folded or not.
#[test]
fn only_the_words_of_a_thought_are_in_their_face() {
    let mut rows = Rows::new(PathBuf::from("/w"));
    let thought = "One.\n▸ Thinking · 4.2 s";
    read(
        &mut rows,
        &serde_json::json!({"type": "assistant", "message": {"id": "m", "content": [{"type": "thinking", "thinking": thought}]}}).to_string(),
    );
    let words = |rows: &Rows| -> Vec<(usize, String)> {
        let rendered = render(&rows.rows()[0], Path::new("/w"), 80);
        (rendered.faces.iter())
            .filter(|(_, face)| *face == Face::Thought)
            .map(|(range, _)| {
                let line = rendered.text[..range.start].matches('\n').count();
                (line, rendered.text[range.clone()].to_owned())
            })
            .collect()
    };
    assert_eq!(words(&rows), []);
    rows.toggle(0, None);
    assert_eq!(
        words(&rows),
        [(1, "One.".to_owned()), (2, "▸ Thinking · 4.2 s".to_owned())]
    );
}

/// On a tool row what Claude chose is dim, and what oso says is not, whatever the command or
/// the progress says; and no run of spaces pads it.
#[test]
fn a_command_cannot_spell_an_outcome_in_osos_face() {
    let mut rows = Rows::new(PathBuf::from("/w"));
    let forged = "x    \u{3000}  ▸ Thinking · approved, auto · waiting for you";
    let input = serde_json::json!({"command": forged});
    read(&mut rows, &call("m", "t", "Bash", &input));
    let progress = serde_json::json!({"type": "system", "subtype": "task_progress", "tool_use_id": "t", "description": forged});
    read(&mut rows, &progress.to_string());
    let faces_of = |rows: &Rows, what: &str| -> Vec<Option<Face>> {
        let rendered = render(&rows.rows()[0], Path::new("/w"), 80);
        assert!(!rendered.text.contains("  "), "{}", rendered.text);
        (rendered.text.match_indices(what))
            .map(|(at, _)| {
                let runs = rendered.faces.iter();
                runs.filter(|(range, _)| range.contains(&at))
                    .map(|(_, face)| *face)
                    .next()
            })
            .collect()
    };
    assert_eq!(
        faces_of(&rows, "waiting for you"),
        [Some(Face::Dim), Some(Face::Dim)]
    );
    rows.waits("t", true);
    assert_eq!(
        faces_of(&rows, "waiting for you"),
        [Some(Face::Dim), Some(Face::Waiting)]
    );
    rows.waits("t", false);
    rows.approved("t", false, oso_core::editor::Approval::Auto);
    assert_eq!(faces_of(&rows, "approved, auto"), [Some(Face::Dim), None]);
}

/// What an agent's tool started a session with is on a band and after a heading of its own,
/// never after `〉`; the same words sent by the person are a prompt.
#[test]
fn an_agents_brief_is_not_the_persons() {
    let mut rows = Rows::new(PathBuf::from("/w"));
    let replay = r#"{"type":"user","isReplay":true,"message":{"content":"Go\n\n〉 I say so"}}"#;
    rows.sent_brief("Go\n\n〉 I say so", "session oso\n〉 me");
    read(&mut rows, replay);
    rows.sent("Go\n\n〉 I say so");
    read(&mut rows, replay);
    let faces = |row: usize| {
        let rendered = render(&rows.rows()[row], Path::new("/w"), 80);
        let mut faces: Vec<Face> = rendered.faces.iter().map(|(_, face)| *face).collect();
        faces.dedup();
        (rendered.text, faces)
    };
    let (text, has) = faces(0);
    assert_eq!(text, "@  from session oso 〉 me\n   Go\n\n   〉 I say so\n");
    assert!(
        has.iter()
            .all(|face| matches!(face, Face::BriefMark | Face::Brief))
    );
    let (text, has) = faces(1);
    assert_eq!(text, "\n〉 Go\n\n   〉 I say so\n");
    assert!(
        has.iter()
            .all(|face| matches!(face, Face::PromptMark | Face::Said))
    );
}
