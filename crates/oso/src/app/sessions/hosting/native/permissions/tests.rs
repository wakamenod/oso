//! Whether a request says the same of a call as the session's stream (ADR 0165, step 2).
//!
//! Must not depend on: a window or a socket; plain values stand in for the rows.

use serde_json::json;

use super::says_the_same;
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
        approved: None,
        allowed: None,
        waiting: false,
    }
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

#[test]
fn a_plans_stream_carries_none_so_its_id_and_tool_match() {
    let stream = tool("ExitPlanMode", &json!({}));
    assert!(says_the_same(&stream, Some(&json!({"plan": "Do it."}))));
    assert!(!says_the_same(&stream, None));
}
