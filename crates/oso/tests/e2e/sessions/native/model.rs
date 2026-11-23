//! A native session's `/model` and `/effort` (ADR 0165, part 3c), with a fake `claude -p` that
//! lists both commands and two models in `initialize`'s answer: choosing `/model` in the
//! commands' prompt completes over the models, `/effort` over the chosen model's levels; each
//! is noted once the CLI's `result` says it set it, the effort shown after the model; and a
//! resume starts with both.
//!
//! Must not depend on: the app's internals; the choice's rules have their own tests.

use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::super::super::harness::App;
use super::super::super::spaces::trust_restricted;
use super::{Flow, in_composer, text};

/// The fake: run `N` from 1 writes its arguments to `run.N`, and answers with the model its
/// `--model` names, else Sonnet. It answers `initialize` with `/model` and `/effort` and two
/// models, Sonnet's levels `low` and `high`, Haiku's `low` and `max`. Each message `M` it writes
/// to `sent.N.M`, and starts a turn whose `system/init` names the model. `/model X` and
/// `/effort X` it answers as the CLI does, with no replay, a synthetic reply and a `result`
/// naming the local command; any other message it replays and answers `reply N.M`.
const MODEL_FAKE: &str = r#"#!/bin/sh
dir='DIR'
n=$(ls "$dir" | grep -c '^run\.')
n=$((n+1))
for a in "$@"; do printf 'arg %s\n' "$a"; done > "$dir/tmp.$n"
mv "$dir/tmp.$n" "$dir/run.$n"
model=claude-sonnet-5-5
for a in "$@"; do
  case "$a" in --model=haiku) model=claude-haiku-5-5 ;; esac
done
local='{"type":"result","subtype":"success","is_error":false,"num_turns":0,"duration_ms":1,"session_id":"s","local_command":"%s","result":"%s"}\n'
m=0
while IFS= read -r line; do
  case "$line" in
    *'"control_request"'*)
      case "$line" in
        *'"initialize"'*)
          printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"oso-0","response":{"current_permission_mode":"default","commands":[{"name":"model","description":"Set the AI model","argumentHint":"[model]","builtin":true},{"name":"effort","description":"Set the effort level","argumentHint":"[level]","builtin":true}],"models":[{"value":"sonnet","resolvedModel":"claude-sonnet-5-5","displayName":"Sonnet 5.5","description":"Most efficient","supportsEffort":true,"supportedEffortLevels":["low","high"]},{"value":"haiku","resolvedModel":"claude-haiku-5-5","displayName":"Haiku 5.5","description":"Fastest","supportsEffort":true,"supportedEffortLevels":["low","max"]}]}}}' ;;
      esac
      continue ;;
  esac
  m=$((m+1))
  said=$(printf '%s\n' "$line" | sed 's/.*"content":"\([^"]*\)".*/\1/')
  printf '%s\n' "$said" > "$dir/sent.$n.$m"
  printf '{"type":"system","subtype":"init","cwd":"/w","session_id":"s","model":"%s","permissionMode":"default"}\n' "$model"
  case "$said" in
    "/model "*)
      chosen=${said#/model }
      case "$chosen" in haiku) model=claude-haiku-5-5 ;; esac
      reply="Set model to $chosen for this session only"
      printf '{"type":"assistant","message":{"id":"m%s%s","model":"<synthetic>","content":[{"type":"text","text":"%s"}]}}\n' "$n" "$m" "$reply"
      printf "$local" model "$reply" ;;
    "/effort "*)
      reply="Set effort level to ${said#/effort } (this session only): made up"
      printf '{"type":"assistant","message":{"id":"m%s%s","model":"<synthetic>","content":[{"type":"text","text":"%s"}]}}\n' "$n" "$m" "$reply"
      printf "$local" effort "$reply" ;;
    *)
      printf '{"type":"user","isReplay":true,"message":{"content":"%s"}}\n' "$said"
      printf '{"type":"assistant","message":{"id":"m%s%s","model":"%s","content":[{"type":"text","text":"reply %s.%s"}]}}\n' "$n" "$m" "$model" "$n" "$m"
      printf '{"type":"result","subtype":"success","duration_ms":10,"session_id":"s"}\n' ;;
  esac
done
"#;

fn modeline(state: &Value) -> &str {
    state["modeline"].as_str().unwrap_or_default()
}

/// The open prompt's name and completion rows, if one is open.
fn prompt(state: &Value) -> Option<(String, Value)> {
    let prompt = state["prompts"].as_array()?.last()?;
    let name = prompt["prompt"].as_str()?.to_owned();
    Some((name, prompt["completion"]["rows"].clone()))
}

/// Opens the commands' prompt with a `/` in the empty composer, once the session listed them,
/// chooses `command` there, and waits for its argument's prompt, `asked`: its rows.
fn argument_prompt(app: &mut App, command: &str, asked: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        app.send_keys("/");
        thread::sleep(Duration::from_millis(200));
        if prompt(&app.state()).is_some_and(|(name, _)| name == "Command:") {
            break;
        }
        assert!(Instant::now() < deadline, "no commands' prompt");
        app.send_keys("DEL");
    }
    let typed: Vec<String> = command.chars().map(String::from).collect();
    app.send_keys(&format!("{} RET", typed.join(" ")));
    let state = app.wait_for(asked, |state| {
        prompt(state).is_some_and(|(name, _)| name == asked)
    });
    prompt(&state).unwrap().1
}

/// The transcript once `seen` holds, then back to the composer.
fn transcript_until(app: &mut App, what: &str, seen: impl Fn(&Value) -> bool) -> Value {
    app.send_keys("C-c C-o");
    let state = app.wait_for(what, |state| {
        state["buffer"]["name"] == "*claude: proj*" && seen(state)
    });
    app.send_keys("i");
    app.wait_for("the composer", in_composer);
    state
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn model_and_effort_complete_their_argument_are_noted_and_a_resume_keeps_them() {
    let flow = Flow::with("native-model", MODEL_FAKE, "");
    let mut app = flow.start();
    app.open_file(&flow.project.join("notes.md"));
    trust_restricted(&mut app, "proj");
    app.wait_for("the project trusted", |state| {
        !modeline(state).contains("restricted")
    });
    app.run_command("claude", None).unwrap();
    app.wait_for("the composer", in_composer);

    // `/model` goes on to the models `initialize` listed; the one chosen waits, unsent.
    let rows = argument_prompt(&mut app, "model", "Model:");
    assert_eq!(rows, serde_json::json!(["sonnet", "haiku"]));
    app.send_keys("h a i RET");
    let state = app.wait_for("the model chosen", |state| {
        prompt(state).is_none() && text(state) == "/model haiku"
    });
    assert!(in_composer(&state), "{state}");
    assert!(!flow.scratch.path().join("sent.1.1").exists());
    app.send_keys("C-c C-c");
    assert_eq!(flow.wait_for_file("sent.1.1").trim(), "/model haiku");
    transcript_until(&mut app, "the model set", |state| {
        text(state).contains("Set model to haiku")
    });

    // `/effort` goes on to the chosen model's levels, not the shown one's.
    let rows = argument_prompt(&mut app, "effort", "Effort:");
    assert_eq!(rows, serde_json::json!(["low", "max"]));
    app.send_keys("m a x RET");
    app.wait_for("the level chosen", |state| text(state) == "/effort max");
    app.send_keys("C-c C-c");
    assert_eq!(flow.wait_for_file("sent.1.2").trim(), "/effort max");
    // Noted from the result: the effort follows the model, which the turn's init names.
    transcript_until(&mut app, "the effort shown", |state| {
        modeline(state).contains("Haiku 5.5 max")
    });

    // Stopped and resumed: the new claude starts with both.
    app.send_keys("C-c C-o");
    app.wait_for("the transcript", |state| {
        state["buffer"]["name"] == "*claude: proj*"
    });
    app.run_command("session-stop", None).unwrap();
    app.wait_for("the session stopped", |state| {
        modeline(state).contains("stopped")
    });
    app.run_command("session-resume", None).unwrap();
    let args = flow.wait_for_file("run.2");
    assert!(args.contains("arg --resume="), "{args}");
    assert!(args.contains("arg --model=haiku"), "{args}");
    assert!(args.contains("arg --effort=max"), "{args}");
    let first = flow.wait_for_file("run.1");
    assert!(
        !first.contains("--model") && !first.contains("--effort"),
        "{first}"
    );
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}
