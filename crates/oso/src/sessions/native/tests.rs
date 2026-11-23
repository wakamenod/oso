//! Native sessions' pure part: the stream recorded from the CLI read, folded into rows and
//! rendered, and each part's edges.
//!
//! Must not depend on: `app`, threads or a process; these tests drive the pure part with
//! recorded lines.
//!
//! The fixtures are recorded from `claude -p` with the flags ADR 0165 names, in an empty
//! directory with `--safe-mode`, then scrubbed: ids, paths, signatures, the account's values
//! and the lists of the person's setup are replaced. One directory per tested CLI version.

use std::path::{Path, PathBuf};

use oso_core::editor::Approval;

use super::render::{Face, render, seconds, tail, tokens};
use super::rows::{Kind, RowChange, Rows, ToolState};
use super::turns::target;
use super::wire::{Event, MAX_LINE, MAX_TEXT, parse, user_message};
use crate::sessions::state::Target;

mod look;

const TOOLS_2_1_330: &str = include_str!("fixtures/2.1.330/tools.jsonl");
/// What the recording's turns sent, as oso would have.
const SENT_2_1_330: [&str; 4] = [
    "Do these in order with your tools, briefly: 1) Read notes.txt. 2) Use the Edit tool to \
     replace 'apple' with 'pear' in notes.txt. 3) Run the shell command: echo hello. 4) Run \
     the shell command: python3 -c 'print(1)'. If anything is denied, just say so in one line.",
    "Use the Agent tool (subagent_type general-purpose) with the prompt 'Count the lines of \
     notes.txt with the Read tool and answer with the number only.' Then tell me the number.",
    "/compact",
    "In one sentence: what fruit is in notes.txt now? Think briefly first.",
];

/// What a line keeps fits in what a line may be.
const _: () = assert!(MAX_TEXT < MAX_LINE);

fn events(lines: &str) -> Vec<Event> {
    lines.lines().map(|line| parse(line.as_bytes())).collect()
}

fn recorded() -> Rows {
    let mut rows = Rows::new(PathBuf::from("/work/probe"));
    for text in SENT_2_1_330 {
        rows.sent(text);
    }
    for event in events(TOOLS_2_1_330) {
        for change in rows.apply(event) {
            assert!(change.row() < rows.rows().len());
        }
    }
    rows
}

fn read(rows: &mut Rows, line: &str) -> Vec<RowChange> {
    rows.apply(parse(line.as_bytes()))
}

/// Every row rendered, as the buffer would hold it.
fn shown(rows: &Rows) -> String {
    rows.rows()
        .iter()
        .map(|row| render(row, rows.directory(), 80).text)
        .collect()
}

fn call(message: &str, id: &str, name: &str, input: &serde_json::Value) -> String {
    serde_json::json!({
        "type": "assistant",
        "message": {"id": message, "content": [
            {"type": "tool_use", "id": id, "name": name, "input": input},
        ]},
    })
    .to_string()
}

#[test]
fn every_recorded_line_reads() {
    for (at, event) in events(TOOLS_2_1_330).into_iter().enumerate() {
        assert!(!matches!(event, Event::Unreadable { .. }), "line {at}");
    }
}

#[test]
fn a_recorded_session_folds_into_rows() {
    insta::assert_snapshot!(shown(&recorded()));
}

#[test]
fn a_recorded_session_unfolded() {
    let mut rows = recorded();
    for at in 0..rows.rows().len() {
        // A change is open already.
        if rows.rows()[at].folded {
            rows.toggle(at, None);
        }
    }
    insta::assert_snapshot!(shown(&rows));
}

#[test]
fn the_recording_says_the_turns() {
    let targets: Vec<Target> = events(TOOLS_2_1_330).iter().filter_map(target).collect();
    let ended = targets
        .iter()
        .filter(|target| **target == Target::Idle { visible: true })
        .count();
    assert_eq!(ended, 4);
    assert_eq!(targets.first(), Some(&Target::Working));
}

#[test]
fn the_session_and_its_rate_limit_read() {
    let events = events(TOOLS_2_1_330);
    let Some(Event::Init(init)) = events.iter().find(|event| matches!(event, Event::Init(_)))
    else {
        panic!("no init")
    };
    assert_eq!(
        (init.cwd.as_str(), init.version.as_str()),
        ("/work/probe", "2.1.330")
    );
    assert!(
        init.capabilities
            .iter()
            .any(|name| name == "interrupt_receipt_v1")
    );
    let Some(Event::RateLimit(limit)) = events
        .iter()
        .find(|event| matches!(event, Event::RateLimit(_)))
    else {
        panic!("no rate limit")
    };
    assert_eq!(
        (limit.status.as_str(), limit.kind.as_str()),
        ("allowed", "seven_day")
    );
    assert_eq!(
        (limit.utilization, limit.resets_at),
        (Some(0.25), Some(1_800_000_000))
    );
}

#[test]
fn a_denied_call_stays_denied_after_its_error_result() {
    let rows = recorded();
    let denied = rows.rows().iter().filter(|row| {
        matches!(&row.kind, Kind::Tools(tools)
            if tools.iter().any(|tool| matches!(tool.state, ToolState::Denied(_))))
    });
    assert_eq!(denied.count(), 1);
}

#[test]
fn a_message_line_is_one_json_line() {
    let line = user_message("a \"quote\"\nand a newline");
    assert!(line.ends_with('\n'));
    assert_eq!(line.matches('\n').count(), 1);
    let value: serde_json::Value = serde_json::from_str(line.trim_end()).unwrap();
    assert_eq!(value["message"]["content"], "a \"quote\"\nand a newline");
    assert_eq!(value["type"], "user");
}

#[test]
fn what_is_not_one_object_is_unreadable() {
    for line in ["", "[]", "\"x\"", "{", "{\"type\":", "not json"] {
        assert_eq!(
            parse(line.as_bytes()),
            Event::Unreadable { bytes: line.len() },
            "{line}"
        );
    }
    let deep = format!("{}{}", "[".repeat(10_000), "]".repeat(10_000));
    let nested = format!("{{\"type\":\"user\",\"message\":{deep}}}");
    assert!(matches!(parse(nested.as_bytes()), Event::Unreadable { .. }));
}

#[test]
fn an_unknown_line_shows_nothing() {
    let mut rows = Rows::new(PathBuf::from("/w"));
    for line in [
        r#"{"type":"keep_alive"}"#,
        r#"{"type":"system","subtype":"hook_started"}"#,
    ] {
        assert!(matches!(parse(line.as_bytes()), Event::Other(_)));
        assert!(read(&mut rows, line).is_empty());
    }
    assert!(rows.rows().is_empty());
}

#[test]
fn a_long_text_is_cut_at_a_character() {
    let text = "é".repeat(MAX_TEXT);
    let line = serde_json::json!({
        "type": "assistant",
        "message": {"id": "m", "content": [{"type": "text", "text": text}]},
    })
    .to_string();
    let Event::Assistant(message) = parse(line.as_bytes()) else {
        panic!("not read")
    };
    let [super::wire::Block::Text(kept)] = message.blocks.as_slice() else {
        panic!("no text")
    };
    assert!(kept.len() <= MAX_TEXT);
    assert!(kept.chars().all(|c| c == 'é'));
}

#[test]
fn an_interrupted_turn_ends_it_and_says_so() {
    let line = r#"{"type":"result","subtype":"error_during_execution","is_error":true,"terminal_reason":"aborted_streaming","duration_ms":1200}"#;
    assert_eq!(
        target(&parse(line.as_bytes())),
        Some(Target::Idle { visible: true })
    );
    let mut rows = Rows::new(PathBuf::from("/w"));
    read(&mut rows, line);
    assert_eq!(
        shown(&rows).trim_start_matches(' '),
        "✗ interrupted · 1.2 s\n\n"
    );
    let failed = "{\"type\":\"result\",\"is_error\":true,\"subtype\":\"x\\ny\",\"terminal_reason\":\"\\u001b\"}";
    let mut rows = Rows::new(PathBuf::from("/w"));
    read(&mut rows, failed);
    assert_eq!(shown(&rows).trim_start_matches(' '), "✗ x y ( )\n\n");
}

#[test]
fn a_turn_that_ends_closes_what_streams() {
    let mut rows = Rows::new(PathBuf::from("/w"));
    let start = r#"{"type":"stream_event","api_message_id":"m","event":{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"t","name":"Bash","input":{}}}}"#;
    let text = r#"{"type":"stream_event","api_message_id":"m","event":{"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}}"#;
    let delta = r#"{"type":"stream_event","api_message_id":"m","event":{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Hel"}}}"#;
    for line in [start, text, delta] {
        read(&mut rows, line);
    }
    let changes = read(&mut rows, r#"{"type":"result","subtype":"success"}"#);
    // The reply shows the same once it stops streaming; the call does not.
    assert_eq!(changes, vec![RowChange::Changed(0), RowChange::Added(2)]);
    assert!(
        matches!(&rows.rows()[0].kind, Kind::Tools(tools) if tools[0].state == ToolState::Unfinished)
    );
    // A task's word after the turn still finds its call.
    let late =
        r#"{"type":"system","subtype":"task_progress","tool_use_id":"t","description":"late"}"#;
    assert_eq!(read(&mut rows, late), vec![RowChange::Changed(0)]);
}

#[test]
fn a_reply_that_grows_is_drawn_from_its_last_line() {
    let mut rows = Rows::new(PathBuf::from("/w"));
    let start = r#"{"type":"stream_event","api_message_id":"m","event":{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}}"#;
    assert_eq!(read(&mut rows, start), vec![RowChange::Added(0)]);
    let mut drawn = render(&rows.rows()[0], rows.directory(), 80).text;
    for piece in ["He", "llo\nwo", "rld\n", "", "\n\nend"] {
        let part = serde_json::json!({"type": "text_delta", "text": piece});
        let event = serde_json::json!({"type": "content_block_delta", "index": 0, "delta": part});
        let delta =
            serde_json::json!({"type": "stream_event", "api_message_id": "m", "event": event});
        for change in rows.apply(parse(delta.to_string().as_bytes())) {
            let RowChange::Grew { row, lines, from } = change else {
                panic!("{change:?}")
            };
            let kept: String = drawn.split_inclusive('\n').take(lines).collect();
            drawn = kept + &tail(&rows.rows()[row], lines, from).text;
        }
        assert_eq!(
            drawn,
            render(&rows.rows()[0], rows.directory(), 80).text,
            "after {piece:?}"
        );
    }
    // A blank line ends in no space.
    assert_eq!(drawn, "  Hello\n  world\n\n\n  end\n");
    let whole = r#"{"type":"assistant","message":{"id":"m","content":[{"type":"text","text":"Hello\nworld\n\n\nend"}]}}"#;
    assert!(read(&mut rows, whole).is_empty());
}

#[test]
fn only_what_oso_sent_is_the_persons() {
    let mut rows = Rows::new(PathBuf::from("/w"));
    rows.sent("/compact");
    rows.sent("Fix it");
    for line in [
        r#"{"type":"user","isReplay":true,"message":{"content":"Fix it"}}"#,
        r#"{"type":"user","message":{"content":"not replayed"}}"#,
        r#"{"type":"user","isReplay":true,"isSynthetic":true,"message":{"content":"a summary"}}"#,
        r#"{"type":"user","isReplay":true,"message":{"content":"<command-name>/x</command-name>"}}"#,
        r#"{"type":"user","isReplay":true,"message":{"content":"<local-command-stderr>no</local-command-stderr>"}}"#,
        r#"{"type":"user","isReplay":true,"message":{"content":"<local-command-stdout>Done </local-command-stdout>"}}"#,
        r#"{"type":"user","isReplay":true,"parent_tool_use_id":"t","message":{"content":"a subagent's prompt"}}"#,
        r#"{"type":"user","isReplay":true,"message":{"content":"Fix it"}}"#,
        r#"{"type":"user","isReplay":true,"message":{"content":"a message from elsewhere"}}"#,
    ] {
        read(&mut rows, line);
    }
    assert_eq!(
        shown(&rows),
        "〉 Fix it\n\n  Done\n\n  Fix it\n\n  a message from elsewhere\n"
    );
}

#[test]
fn a_target_is_one_short_line() {
    let mut rows = Rows::new(PathBuf::from("/w"));
    let input = serde_json::json!({"command": "echo a\u{1b}[31m\nrm -rf ~"});
    read(&mut rows, &call("m", "a", "Bash", &input));
    let padded =
        serde_json::json!({"command": format!("echo hi {}; rm -rf ~", " ".repeat(70_000))});
    read(&mut rows, &call("m", "b", "Bash", &padded));
    let named = serde_json::json!({"file_path": "/w/src/a.rs"});
    read(&mut rows, &call("m", "c", "x\n› forged", &named));
    let lines: Vec<String> = rows
        .rows()
        .iter()
        .map(|row| render(row, Path::new("/w"), 80).text)
        .collect();
    assert_eq!(lines[0], "… $ Bash · echo a [31m rm -rf ~\n");
    assert!(lines[1].starts_with("… $ Bash · echo hi "));
    assert!(lines[1].ends_with("… · input cut\n"));
    assert_eq!(lines[2], "… * x › forged · /w/src/a.rs\n");
}

#[test]
fn reads_one_after_another_share_a_row() {
    let mut rows = Rows::new(PathBuf::from("/w"));
    let mut changes = Vec::new();
    for (id, path) in [("a", "/w/a.rs"), ("b", "/w/b.rs"), ("c", "/w/c.rs")] {
        let input = serde_json::json!({"file_path": path});
        changes.extend(read(&mut rows, &call(id, id, "Read", &input)));
    }
    assert_eq!(
        changes,
        vec![
            RowChange::Added(0),
            RowChange::Changed(0),
            RowChange::Changed(0)
        ]
    );
    let result = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"b","is_error":true,"content":"gone"}]}}"#;
    assert_eq!(read(&mut rows, result), vec![RowChange::Changed(0)]);
    assert_eq!(
        render(&rows.rows()[0], rows.directory(), 80).text,
        "… ≡ Read ×3 · a.rs, b.rs, c.rs · 1 failed\n"
    );
    rows.toggle(0, None);
    rows.toggle(0, Some(1));
    let unfolded = render(&rows.rows()[0], rows.directory(), 80);
    assert!(
        unfolded
            .text
            .contains("\n│ … ≡ Read · a.rs\n│ ✗ ≡ Read · b.rs · error\n│     gone\n")
    );
    let (slot, range) = &unfolded.calls[1];
    assert_eq!(
        (*slot, &unfolded.text[range.clone()]),
        (1, "│ ✗ ≡ Read · b.rs · error\n│     gone\n")
    );
}

#[test]
fn a_read_outside_the_project_keeps_a_row_of_its_own() {
    let mut rows = Rows::new(PathBuf::from("/w"));
    for (id, path) in [
        ("a", "/w/a.rs"),
        ("b", "/home/me/.ssh/id_ed25519"),
        ("c", "/w/.env"),
        ("d", "/w/../etc/passwd"),
        ("e", "/w/b.rs"),
    ] {
        let input = serde_json::json!({"file_path": path});
        read(&mut rows, &call(id, id, "Read", &input));
    }
    assert_eq!(rows.rows().len(), 5);
    let glob = serde_json::json!({"pattern": "/etc/*"});
    read(&mut rows, &call("f", "f", "Glob", &glob));
    let glob = serde_json::json!({"pattern": "*.rs"});
    read(&mut rows, &call("g", "g", "Glob", &glob));
    assert_eq!(rows.rows().len(), 7);
}

#[test]
fn a_streamed_read_that_may_not_share_leaves_its_row() {
    let mut rows = Rows::new(PathBuf::from("/w"));
    for (index, id) in [(0, "a"), (1, "b")] {
        let block = serde_json::json!({"type": "tool_use", "id": id, "name": "Read"});
        let event = serde_json::json!({"type": "content_block_start", "index": index, "content_block": block});
        let start =
            serde_json::json!({"type": "stream_event", "api_message_id": "m", "event": event});
        read(&mut rows, &start.to_string());
    }
    assert_eq!(rows.rows().len(), 1);
    let inside = serde_json::json!({"file_path": "/w/a.rs"});
    assert_eq!(
        read(&mut rows, &call("m", "a", "Read", &inside)),
        vec![RowChange::Changed(0)]
    );
    let outside = serde_json::json!({"file_path": "/etc/passwd"});
    let changes = read(&mut rows, &call("m", "b", "Read", &outside));
    assert_eq!(changes, vec![RowChange::Changed(0), RowChange::Added(1)]);
    let result = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"b","content":"root"}]}}"#;
    assert_eq!(read(&mut rows, result), vec![RowChange::Changed(1)]);
    assert_eq!(
        render(&rows.rows()[1], rows.directory(), 80).text,
        "▸ ≡ Read · /etc/passwd  1 line\n"
    );
}

#[test]
fn many_results_for_one_call_change_its_row_once() {
    let mut rows = Rows::new(PathBuf::from("/w"));
    read(&mut rows, &call("m", "t", "Edit", &serde_json::json!({})));
    let results: Vec<serde_json::Value> = (0..1024)
        .map(|_| serde_json::json!({"type": "tool_result", "tool_use_id": "t", "content": "x"}))
        .collect();
    let line = serde_json::json!({"type": "user", "message": {"content": results}});
    assert_eq!(
        read(&mut rows, &line.to_string()),
        vec![RowChange::Changed(0)]
    );
}

#[test]
fn a_patch_keeps_within_its_bounds() {
    let hunks: Vec<serde_json::Value> = (0..100_000).map(|_| serde_json::json!({})).collect();
    let line = serde_json::json!({
        "type": "user",
        "message": {"content": [{"type": "tool_result", "tool_use_id": "t", "content": ""}]},
        "tool_use_result": {"structuredPatch": hunks},
    });
    let Event::User(user) = parse(line.to_string().as_bytes()) else {
        panic!("not read")
    };
    assert!(user.results[0].patch.len() <= super::wire::MAX_DIFF_LINES);
    let lines = vec!["+".repeat(16 << 10); 200];
    let line = serde_json::json!({
        "type": "user",
        "message": {"content": [{"type": "tool_result", "tool_use_id": "t", "content": ""}]},
        "tool_use_result": {"structuredPatch": [{"lines": lines}]},
    });
    let Event::User(user) = parse(line.to_string().as_bytes()) else {
        panic!("not read")
    };
    let bytes: usize = user.results[0].patch[0].lines.iter().map(String::len).sum();
    assert!(bytes <= super::wire::MAX_DIFF_BYTES);
    // Only an edit's result shows a diff.
    let mut rows = Rows::new(PathBuf::from("/w"));
    let input = serde_json::json!({"command": "x"});
    read(&mut rows, &call("m", "t", "Bash", &input));
    rows.apply(Event::User(user));
    let Kind::Tools(tools) = &rows.rows()[0].kind else {
        panic!("no call")
    };
    assert!(
        tools[0]
            .result
            .as_ref()
            .is_some_and(|result| result.patch.is_empty())
    );
}

#[test]
fn faces_cover_the_text_they_name() {
    let mut rows = recorded();
    for at in 0..rows.rows().len() {
        // A change is open already.
        if rows.rows()[at].folded {
            rows.toggle(at, None);
        }
    }
    for row in rows.rows() {
        let rendered = render(row, rows.directory(), 80);
        assert!(rendered.text.ends_with('\n'));
        for (range, _) in &rendered.faces {
            assert!(range.start < range.end && range.end <= rendered.text.len());
            assert!(rendered.text.is_char_boundary(range.start));
            assert!(rendered.text.is_char_boundary(range.end));
        }
    }
    let diff = rows
        .rows()
        .iter()
        .map(|row| render(row, rows.directory(), 80))
        .find(|rendered| rendered.faces.iter().any(|(_, face)| *face == Face::Added));
    assert!(diff.is_some());
}

#[test]
fn a_note_of_osos_own_is_a_row_that_does_not_fold() {
    let mut rows = Rows::new(PathBuf::from("/w"));
    assert_eq!(rows.note("Resumed"), RowChange::Added(0));
    assert_eq!(rows.toggle(0, None), None);
    assert_eq!(rows.toggle(1, None), None);
    assert_eq!(shown(&rows), "  Resumed\n");
}

#[test]
fn counts_and_durations_read_as_the_cli_says_them() {
    assert_eq!(tokens(999), "999");
    assert_eq!(tokens(20_215), "20.2k");
    assert_eq!(seconds(850), "850 ms");
    assert_eq!(seconds(4730), "4.7 s");
    assert_eq!(seconds(125_000), "2 min 5 s");
}

#[test]
fn a_waiting_call_is_one_whose_whole_message_came_and_that_has_no_result() {
    let mut rows = Rows::new(PathBuf::from("/w"));
    let start = r#"{"type":"stream_event","api_message_id":"m","event":{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"t","name":"Bash","input":{}}}}"#;
    read(&mut rows, start);
    // Its input still streams: nothing to show the person yet.
    assert!(rows.waiting_call("t").is_none());
    read(
        &mut rows,
        &call("m", "t", "Bash", &serde_json::json!({"command": "ls"})),
    );
    let tool = rows.waiting_call("t").unwrap();
    assert_eq!(tool.name.as_str(), "Bash");
    assert_eq!(tool.input.text.as_deref(), Some("ls"));
    assert!(rows.waiting_call("other").is_none());
    let result = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t","content":"ok"}]}}"#;
    read(&mut rows, result);
    assert!(rows.waiting_call("t").is_none());
}

/// A plan the person approved on its card says how on its row (ADR 0165, part 3d), as it runs
/// and once its result came, but not once it failed.
#[test]
fn an_approved_plan_says_how_on_its_row() {
    let mut rows = Rows::new(PathBuf::from("/w"));
    let call = r#"{"type":"assistant","message":{"id":"m","content":[{"type":"tool_use","id":"p","name":"ExitPlanMode","input":{}}]}}"#;
    read(&mut rows, call);
    assert_eq!(
        rows.approved("p", true, Approval::Auto),
        Some(RowChange::Changed(0))
    );
    assert!(
        shown(&rows).contains("ExitPlanMode · approved, edited, auto"),
        "{}",
        shown(&rows)
    );
    let done = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"p","content":"User has approved your plan."}]}}"#;
    read(&mut rows, done);
    assert!(
        shown(&rows).contains("· approved, edited, auto"),
        "{}",
        shown(&rows)
    );
    assert_eq!(rows.approved("none", false, Approval::Manual), None);
    let failed = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"q","is_error":true,"content":"no"}]}}"#;
    let other = r#"{"type":"assistant","message":{"id":"n","content":[{"type":"tool_use","id":"q","name":"ExitPlanMode","input":{}}]}}"#;
    read(&mut rows, other);
    rows.approved("q", false, Approval::AcceptEdits);
    read(&mut rows, failed);
    assert!(
        shown(&rows).contains("ExitPlanMode · error"),
        "{}",
        shown(&rows)
    );
    rows.approved("p", false, Approval::AcceptEdits);
    assert!(shown(&rows).contains("· approved, accept edits"));
}
