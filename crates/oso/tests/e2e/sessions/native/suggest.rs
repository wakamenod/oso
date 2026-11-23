//! A native session's prompt suggestion and recap (ADR 0165, part 3f), with a fake `claude -p`
//! that ends each turn with a reply, its result and a `prompt_suggestion`, as the CLI does with
//! `--prompt-suggestions` after a conversation's first turn.
//!
//! Must not depend on: the app's internals; the placeholder that shows the suggestion, which
//! nothing over the socket reads, the first sentence and the escapes have their own tests.

use std::fs;
use std::time::{Duration, Instant};

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

/// Waits until the composer holds `wanted`, with its text and the echo area if it never does.
fn holds(app: &mut App, wanted: &str) -> Value {
    let end = Instant::now() + Duration::from_secs(20);
    loop {
        let state = app.state();
        if in_composer(&state) && text(&state) == wanted {
            return state;
        }
        let (name, echo) = (&state["buffer"]["name"], &state["echo"]);
        let held = text(&state);
        assert!(
            Instant::now() < end,
            "{name} holds {held:?}, not {wanted:?}; {echo}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Presses `C-c C-s` until the suggestion, which comes after the turn's end, is taken and the
/// composer holds `wanted`. Until then the key puts nothing in.
fn take(app: &mut App, wanted: &str) -> Value {
    let end = Instant::now() + Duration::from_secs(20);
    loop {
        app.send_keys("C-c C-s");
        let state = app.state();
        // Taken once: another press would put it in again.
        if in_composer(&state) && !text(&state).is_empty() {
            return holds(app, wanted);
        }
        assert!(Instant::now() < end, "not taken: {state}");
        std::thread::sleep(Duration::from_millis(100));
    }
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
fn a_suggestion_goes_into_the_composer_escaped_and_a_turn_ended_unseen_is_recapped() {
    let flow = Flow::new("native-suggest");
    let mut app = first_turn(&flow);

    // `C-c C-s` in the transcript puts it into the composer, escaped, not sent, point there
    // after it.
    app.wait_for("the transcript", |state| {
        state["buffer"]["name"] == "*claude: proj*" && text(state).contains("Done 1.")
    });
    take(&mut app, r" /review \@notes.md");
    // No row of the transcript, and nothing else the socket reads, holds it.
    app.send_keys("C-c C-o");
    let state = app.wait_for("the transcript", |state| {
        state["buffer"]["name"] == "*claude: proj*"
    });
    assert!(!state.to_string().contains("/review"), "{state}");

    // Cleared, `M-n` past the newest message puts it in too; with a draft, `C-c C-s` puts it at
    // the draft's end.
    app.send_keys("i");
    app.wait_for("the composer", in_composer);
    app.send_keys("C-x h C-w");
    holds(&mut app, "");
    app.send_keys("M-n");
    holds(&mut app, r" /review \@notes.md");
    app.send_keys("C-x h C-w");
    // Apart: keys sent together run before the host puts it in, as no keys of a person's do but
    // a macro's and those held while a command waits (ADR 0165, part 3f).
    app.send_keys("s e e C-a C-c C-s");
    app.send_keys("x");
    holds(&mut app, r"see /review \@notes.mdx");
    // Nothing goes after a command.
    app.send_keys("C-x h C-w");
    app.send_keys("! l C-c C-s");
    app.wait_for("the draft left as it is", |state| {
        in_composer(state)
            && text(state) == "!l"
            && state["echo"]
                .as_str()
                .is_some_and(|echo| echo.contains("command"))
    });
    app.send_keys("C-x h C-w");
    take(&mut app, r" /review \@notes.md");
    assert!(!flow.scratch.path().join("sent.2").exists(), "not sent");

    // Sent, it is gone.
    app.send_keys("C-c C-c");
    assert_eq!(
        flow.wait_for_file("sent.2").trim_end(),
        r" /review \\@notes.md"
    );
    app.send_keys("C-c C-s");
    app.wait_for("no suggestion", |state| {
        in_composer(state)
            && text(state).is_empty()
            && (state["echo"].as_str()).is_some_and(|echo| echo.contains("No suggestion"))
    });

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
    assert!(text(&state).contains("Done 2. More text."), "{state}");
    take(&mut app, "run the tests");
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}
