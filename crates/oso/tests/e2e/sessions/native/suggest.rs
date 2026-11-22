//! A native session's prompt suggestion and recap (ADR 0165, part 3f), with a fake `claude -p`
//! that ends each turn with a reply, its result and a `prompt_suggestion`, as the CLI does with
//! `--prompt-suggestions` after a conversation's first turn.
//!
//! Must not depend on: the app's internals; the suggestion's row, the first sentence and the
//! escapes have their own tests.

use std::fs;

use serde_json::{Value, json};

use super::super::super::harness::App;
use super::super::super::spaces::trust_restricted;
use super::super::super::worktrees::keys;
use super::{Flow, MESSAGE, in_composer, text};

/// The fake: for each line of its input, `N` from 1, it writes the message's text to
/// `sent.N`, replays it, and once the flow writes `release.N`, replies `Done N. More text.`,
/// ends the turn and suggests `suggest.N`'s text. It exits once its input ends.
const SUGGEST_FAKE: &str = r#"#!/bin/sh
dir='DIR'
cat "$dir/stream.jsonl"
n=0
while IFS= read -r line; do
  case "$line" in *'"control_request"'*) continue ;; esac
  n=$((n+1))
  said=$(printf '%s\n' "$line" | sed 's/.*"content":"\([^"]*\)".*/\1/')
  printf '%s\n' "$said" > "$dir/sent.$n"
  printf '{"type":"user","isReplay":true,"message":{"content":"%s"}}\n' "$said"
  while [ ! -f "$dir/release.$n" ]; do sleep 0.1; done
  printf '{"type":"assistant","message":{"id":"m%s","content":[{"type":"text","text":"Done %s. More text."}]}}\n' "$n" "$n"
  printf '{"type":"result","subtype":"success","duration_ms":10}\n'
  printf '{"type":"prompt_suggestion","suggestion":"%s","uuid":"u%s","session_id":"s"}\n' "$(cat "$dir/suggest.$n")" "$n"
done
"#;

/// The rows the transcript shows, read from it, whatever window is selected.
fn transcript(app: &mut App) -> Value {
    app.send_keys("C-c C-o");
    app.wait_for("the transcript", |state| {
        state["buffer"]["name"] == "*claude: proj*"
    })
}

/// `flow`'s app, with a native session sent [`MESSAGE`], its first turn ended.
fn first_turn(flow: &Flow) -> App {
    let fake = flow.scratch.path().join("fake-claude");
    let script = SUGGEST_FAKE.replace("DIR", &flow.scratch.path().display().to_string());
    fs::write(&fake, script).unwrap();
    let init = r#"{"type":"system","subtype":"init","cwd":"/w","session_id":"s","model":"m"}"#;
    flow.scratch
        .write("stream.jsonl", format!("{init}\n").as_bytes());
    flow.scratch.write("suggest.1", b"/review @notes.md");
    flow.scratch.write("suggest.2", b"run the tests");
    let mut app = flow.start();
    app.open_file(&flow.project.join("notes.md"));
    trust_restricted(&mut app, "proj");
    app.wait_for("the project trusted", |state| {
        state["modeline"]
            .as_str()
            .is_some_and(|modeline| !modeline.contains("restricted"))
    });
    app.run_command("claude", None).unwrap();
    app.wait_for("the composer", in_composer);
    app.send_keys(&keys(MESSAGE));
    app.send_keys("C-c C-c");
    app.send_keys("C-c C-o");
    assert_eq!(flow.wait_for_file("sent.1").trim(), MESSAGE);
    flow.scratch.write("release.1", b"");
    app
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_suggestion_shows_goes_into_the_composer_escaped_and_a_turn_ended_unseen_is_recapped() {
    let flow = Flow::new("native-suggest");
    let mut app = first_turn(&flow);

    // The suggestion, dim at the transcript's end, after the turn's rows.
    let state = app.wait_for("the suggestion", |state| {
        text(state).contains("Suggested: /review @notes.md")
    });
    assert!(
        text(&state)
            .trim_end()
            .ends_with("Suggested: /review @notes.md"),
        "{state}"
    );

    // `M-n` past the newest message, in the empty composer, puts it in escaped, not sent.
    app.send_keys("i");
    app.wait_for("the composer", in_composer);
    app.send_keys("M-n");
    app.wait_for("the suggestion in the composer", |state| {
        in_composer(state) && text(state) == r" /review \@notes.md"
    });
    assert!(!flow.scratch.path().join("sent.2").exists(), "not sent");

    // Sent, it goes from the transcript.
    app.send_keys("C-c C-c");
    assert_eq!(
        flow.wait_for_file("sent.2").trim_end(),
        r" /review \\@notes.md"
    );
    let shown = text(&transcript(&mut app));
    assert!(!shown.contains("Suggested:"), "{shown}");

    // The turn ends while the person is elsewhere; back in the session, the recap is said in
    // the echo area, withheld from what the socket reads, so its modeline is too.
    app.send_keys("C-x b");
    app.send_keys(&keys("notes.md"));
    app.send_keys("RET");
    app.wait_for("away", |state| state["buffer"]["name"] == "notes.md");
    flow.scratch.write("release.2", b"");
    // The turn ended out of sight: the modeline counts the session done (ADR 0121).
    app.wait_for("the turn ended unseen", |state| {
        state["modeline"]
            .as_str()
            .is_some_and(|modeline| modeline.contains("● 1"))
    });
    // A display with no window manager, as Xvfb on CI, may say the window lost the keyboard
    // and never give it back, and no session is in front of the person then: the window
    // takes it, as a click on it would.
    app.call("focus", Some(json!({ "focused": true })));
    app.send_keys("C-x b");
    app.send_keys(&keys("*claude: proj*"));
    app.send_keys("RET");
    let state = app.wait_for("the recap", |state| {
        state["buffer"]["name"] == "*claude: proj*" && state["modeline"].is_null()
    });
    assert!(state["echo"].is_null(), "{state}");
    // The next key ends it; the new suggestion is there.
    app.send_keys("C-l");
    let state = app.wait_for("the recap gone", |state| state["modeline"].is_string());
    assert!(text(&state).contains("Suggested: run the tests"), "{state}");
    assert!(text(&state).contains("Done 2. More text."), "{state}");
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}
