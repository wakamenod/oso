//! Remote Control of a native session (ADR 0181, part RC-2), with a fake `claude -p` that connects to
//! nothing: it answers `remote_control` as the CLI's recorded lines do, says the states a flow
//! writes, and plays the phone's lines. The question and its report; the unblocked respawn and what
//! is asked; the mark; a message and a mode from the phone; off, idle and during a turn; what
//! refuses; and a reconnection across `oso-restart`, with the same session there and another.
//!
//! Must not depend on: the app's internals; the rules have their own tests.

use std::fs;
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

use super::super::super::harness::{App, sockets};
use super::super::super::spaces::trust_restricted;
use super::modes::{footer, modeline, send};
use super::restart::{show_transcript, transcript_shown_again};
use super::{Flow, git_init, in_composer, remote_control_off, text};

/// The fake: run `N` from 1 writes its arguments to `run.N`, and answers `initialize` with
/// `remote_control_available` as its `--settings` allow. `remote_control` on it writes to `on.N`,
/// answers with the session id in `there`, and says that it connected; off it writes to `off.N`.
/// Each message `M` it writes to `sent.N.M`, and, unless it starts with `long`, ends its turn. What
/// the flow puts in `say.N` it prints, as if the phone had caused it. With `slow` there, it takes
/// 2 s to end.
const REMOTE_FAKE: &str = r#"#!/bin/sh
dir='DIR'
n=$(ls "$dir" | grep -c '^run\.')
n=$((n+1))
for a in "$@"; do printf 'arg %s\n' "$a"; done > "$dir/tmp.$n"
mv "$dir/tmp.$n" "$dir/run.$n"
available=true
for a in "$@"; do
  case "$a" in --settings=*'"disableRemoteControl":true'*) available=false ;; esac
done
(
  while :; do
    if [ -f "$dir/say.$n" ]; then
      mv "$dir/say.$n" "$dir/said.$n"
      cat "$dir/said.$n"
    fi
    sleep 0.05
  done
) &
phone=$!
trap 'kill $phone 2>/dev/null' EXIT
trap 'kill $phone 2>/dev/null; [ -f "$dir/slow" ] && sleep 2; exit 0' TERM INT
m=0
while IFS= read -r line; do
  case "$line" in
    *'"control_request"'*)
      id=$(printf '%s\n' "$line" | sed 's/.*"request_id":"\([^"]*\)".*/\1/')
      case "$line" in
        *'"initialize"'*)
          printf '{"type":"control_response","response":{"subtype":"success","request_id":"%s","response":{"current_permission_mode":"default","remote_control_available":%s}}}\n' "$id" "$available" ;;
        *'"remote_control"'*'"enabled":true'* | *'"enabled":true'*'"remote_control"'*)
          printf '%s\n' "$line" > "$dir/tmp.on.$n"
          mv "$dir/tmp.on.$n" "$dir/on.$n"
          there=$(cat "$dir/there")
          printf '{"type":"control_response","response":{"subtype":"success","request_id":"%s","response":{"session_url":"https://claude.ai/code/session_a1","bridge_session_id":"%s"}}}\n' "$id" "$there"
          printf '{"type":"system","subtype":"bridge_state","state":"ready"}\n'
          printf '{"type":"system","subtype":"bridge_state","state":"connected"}\n' ;;
        *'"remote_control"'*)
          printf '%s\n' "$line" > "$dir/off.$n"
          printf '{"type":"control_response","response":{"subtype":"success","request_id":"%s","response":{}}}\n' "$id" ;;
        *'"set_permission_mode"'*)
          asked=$(printf '%s\n' "$line" | sed 's/.*"mode":"\([^"]*\)".*/\1/')
          printf '%s\n' "$asked" > "$dir/asked.$n"
          printf '{"type":"control_response","response":{"subtype":"success","request_id":"%s","response":{"mode":"%s"}}}\n' "$id" "$asked"
          printf '{"type":"system","subtype":"status","status":null,"permissionMode":"%s"}\n' "$asked" ;;
      esac
      continue ;;
  esac
  m=$((m+1))
  said=$(printf '%s\n' "$line" | sed 's/.*"content":"\([^"]*\)".*/\1/')
  printf '%s\n' "$said" > "$dir/sent.$n.$m"
  printf '{"type":"system","subtype":"init","cwd":"/w","session_id":"s","permissionMode":"default","capabilities":[]}\n'
  printf '{"type":"user","isReplay":true,"message":{"content":"%s"}}\n' "$said"
  case "$said" in
    long*) ;;
    *)
      printf '{"type":"assistant","message":{"id":"m%s%s","content":[{"type":"text","text":"reply %s.%s"}]}}\n' "$n" "$m" "$n" "$m"
      printf '{"type":"result","subtype":"success","duration_ms":10}\n' ;;
  esac
done
"#;

/// A flow whose session ended one turn, idle, with the composer selected.
fn idle(test: &str) -> (Flow, App) {
    let flow = Flow::with(test, REMOTE_FAKE, "");
    flow.scratch.write("there", b"cse_a1");
    let mut app = flow.start();
    app.open_file(&flow.project.join("notes.md"));
    trust_restricted(&mut app, "proj");
    app.wait_for("the project trusted", |state| {
        !modeline(state).contains("restricted")
    });
    app.run_command("claude", None).unwrap();
    app.wait_for("the composer", in_composer);
    send(&mut app, "one");
    assert_eq!(flow.wait_for_file("sent.1.1").trim(), "one");
    transcript_says(&mut app, "reply 1.1");
    app.send_keys("i");
    app.wait_for("the composer", in_composer);
    app.try_call("focus", Some(json!({ "focused": true })))
        .unwrap();
    (flow, app)
}

/// Whether the question is open and takes its answer.
fn takes_answer(state: &Value) -> bool {
    state["card"]["question_takes_answer"] == true
}

/// Connects the session: run 2, which asked.
fn connected(flow: &Flow, app: &mut App) {
    app.run_command("native-remote-control", None).unwrap();
    app.wait_for("the question", takes_answer);
    app.send_keys("y");
    flow.wait_for_file("on.2");
    transcript_says(app, "Remote Control is on");
    app.wait_for("the mark", |state| modeline(state).contains("⇄ claude.ai"));
}

fn args(flow: &Flow, run: &str) -> Vec<String> {
    (flow.wait_for_file(run).lines())
        .filter_map(|line| line.strip_prefix("arg "))
        .map(str::to_owned)
        .collect()
}

fn there(flow: &Flow, name: &str) -> bool {
    flow.scratch.path().join(name).exists()
}

/// Waits until `app`'s transcript says `row`, its lines wrapped anywhere.
fn transcript_says(app: &mut App, row: &str) {
    app.send_keys("C-c C-o");
    app.wait_for(row, |state| {
        let rows = text(state);
        let words: Vec<&str> = rows
            .split_whitespace()
            .filter(|word| *word != "↩")
            .collect();
        state["buffer"]["name"] == "*claude: proj*" && words.join(" ").contains(row)
    });
}

/// Waits until the echo area says `words`.
fn echoes(app: &mut App, words: &str) {
    app.wait_for(words, |state| {
        state["echo"]
            .as_str()
            .is_some_and(|echo| echo.contains(words))
    });
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn it_is_turned_on_at_the_persons_yes_marked_and_turned_off() {
    let (flow, mut app) = idle("native-remote");
    assert!(remote_control_off(&args(&flow, "run.1")));
    // The question, with what is sent in the session's window; n turns nothing on.
    app.run_command("native-remote-control", None).unwrap();
    let state = app.wait_for("the question", takes_answer);
    let question = state["minibuffer"].to_string() + &state["prompts"].to_string();
    assert!(question.contains("to claude.ai"), "{state}");
    assert!(
        state.to_string().contains("goes to Anthropic's servers"),
        "{state}"
    );
    app.send_keys("n");
    app.wait_for("the composer again", in_composer);
    thread::sleep(Duration::from_millis(300));
    assert!(!there(&flow, "run.2"));

    connected(&flow, &mut app);
    let run = args(&flow, "run.2");
    assert!(!remote_control_off(&run), "{run:?}");
    assert!(
        run.iter().any(|arg| arg.starts_with("--resume=")),
        "{run:?}"
    );
    let settings = run.iter().find(|arg| arg.starts_with("--settings="));
    assert!(
        settings.is_some_and(|s| s.contains(r#""remoteControlAtStartup":false"#)),
        "{run:?}"
    );
    let asked: Value = serde_json::from_str(&flow.wait_for_file("on.2")).unwrap();
    assert_eq!(asked["request"]["keep_session_on_exit"], true);
    assert_eq!(asked["request"]["name"], "proj");
    assert_eq!(asked["request"]["reattach_session_id"], Value::Null);
    // The mark in the list too.
    app.run_command("sessions", None).unwrap();
    app.wait_for("the list", |state| {
        state["sessions"]["list"]
            .as_str()
            .is_some_and(|list| list.contains("⇄ proj"))
    });
    show_transcript(&mut app);

    // Off, idle: told, ended and resumed blocked; nothing of the mark stays.
    app.run_command("native-remote-control", None).unwrap();
    flow.wait_for_file("off.2");
    assert!(remote_control_off(&args(&flow, "run.3")));
    transcript_says(&mut app, "Remote Control is off");
    app.wait_for("no mark", |state| !modeline(state).contains('⇄'));
    thread::sleep(Duration::from_millis(300));
    assert!(!there(&flow, "on.3"), "a blocked process is never asked");
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn the_phones_message_and_mode_are_shown_and_never_the_persons() {
    let (flow, mut app) = idle("native-remote-phone");
    connected(&flow, &mut app);
    let uuid = "00000000-0000-4000-8000-00000000000a";
    let lines = [
        json!({"type": "command_lifecycle", "command_uuid": uuid, "state": "queued"}),
        json!({
            "type": "user",
            "isReplay": true,
            "uuid": uuid,
            "origin": {"kind": "human"},
            "message": {"content": "from the phone"},
        }),
        json!({"type": "system", "subtype": "status", "permissionMode": "plan"}),
        json!({"type": "result", "subtype": "success", "duration_ms": 10}),
    ];
    let lines: Vec<String> = lines.iter().map(Value::to_string).collect();
    flow.scratch
        .write("say.2", format!("{}\n", lines.join("\n")).as_bytes());
    transcript_says(&mut app, "From claude.ai, not from oso: from the phone");
    transcript_says(
        &mut app,
        "The permission mode changed, by no key here: plan.",
    );
    // Beside the mark, which a narrow modeline keeps.
    app.wait_for("the mode", |state| {
        modeline(state).contains("⇄ plan: claude.ai")
            && footer(state).starts_with("⏸ plan mode on · set by no key here")
    });
    // The person's key goes to default first, not on to auto.
    app.send_keys("S-TAB");
    assert_eq!(flow.wait_for_file("asked.2").trim(), "default");
    // Nor is it in the composer's history.
    app.send_keys("i");
    app.wait_for("the composer", in_composer);
    app.send_keys("M-p");
    let state = app.wait_for("the history", |state| text(state).contains("one"));
    assert!(!text(&state).contains("phone"), "{state}");
    // The next process is not given what the phone chose.
    flow.scratch.write(
        "say.2",
        b"{\"type\":\"system\",\"subtype\":\"status\",\"permissionMode\":\"auto\"}\n",
    );
    transcript_says(&mut app, "by no key here: auto.");
    app.run_command("native-remote-control", None).unwrap();
    let run = args(&flow, "run.3");
    assert!(
        run.iter().any(|arg| arg == "--permission-mode=default"),
        "{run:?}"
    );
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn off_during_a_turn_is_told_at_once_and_ends_at_a_second_press() {
    let (flow, mut app) = idle("native-remote-turn");
    connected(&flow, &mut app);
    app.send_keys("i");
    app.wait_for("the composer", in_composer);
    send(&mut app, "long two");
    flow.wait_for_file("sent.2.1");
    app.run_command("native-remote-control", None).unwrap();
    flow.wait_for_file("off.2");
    transcript_says(&mut app, "in 60 s at the latest");
    app.wait_for("the mark", |state| modeline(state).contains("⇄ going off"));
    // No line takes the mark back.
    flow.scratch.write(
        "say.2",
        b"{\"type\":\"system\",\"subtype\":\"bridge_state\",\"state\":\"connected\"}\n",
    );
    flow.wait_for_file("said.2");
    thread::sleep(Duration::from_millis(300));
    assert!(modeline(&app.state()).contains("⇄ going off"));
    assert!(!there(&flow, "run.3"), "its turn goes on");
    app.run_command("native-remote-control", None).unwrap();
    assert!(remote_control_off(&args(&flow, "run.3")));
    transcript_says(&mut app, "Remote Control is off");
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_stopped_session_a_turn_and_a_remotes_password_refuse() {
    let (flow, mut app) = idle("native-remote-refused");
    app.send_keys("C-c C-o");
    app.wait_for("the transcript", |state| {
        state["buffer"]["name"] == "*claude: proj*"
    });
    app.run_command("session-stop", None).unwrap();
    app.wait_for("stopped", |state| modeline(state).contains("stopped"));
    app.run_command("native-remote-control", None).unwrap();
    echoes(&mut app, "runs no claude: resume it first");
    app.run_command("session-resume", None).unwrap();
    flow.wait_for_file("run.2");
    app.wait_for("the composer", in_composer);
    // Once its process runs: it is refused, as starting, till then.
    app.send_keys("C-c C-o");
    app.wait_for("the session running", |state| {
        let line = modeline(state);
        state["buffer"]["name"] == "*claude: proj*"
            && !line.contains("starting")
            && !line.contains("stopped")
    });
    app.send_keys("i");
    app.wait_for("the composer", in_composer);
    // A remote's address with a password: said without it, and nothing respawns.
    let config = flow.project.join(".git/config");
    let mut text = fs::read_to_string(&config).unwrap();
    text.push_str("[remote \"origin\"]\n\turl = https://me:hunter2@example.invalid/r\n");
    fs::write(&config, &text).unwrap();
    app.run_command("native-remote-control", None).unwrap();
    echoes(&mut app, "a git remote's address holds a password");
    let state = app.state();
    assert!(!state.to_string().contains("hunter2"), "{state}");
    thread::sleep(Duration::from_millis(300));
    assert!(!there(&flow, "run.3"));
    // A turn under way.
    app.wait_for("the composer", in_composer);
    send(&mut app, "long two");
    flow.wait_for_file("sent.2.1");
    app.run_command("native-remote-control", None).unwrap();
    echoes(&mut app, "has a turn under way");
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}

/// Restarts the connected session's oso; `there` is the session the fake answers then.
/// The instance going is the caller's to drop last, as the new one has its log.
fn restarted(test: &str, there: &str) -> (Flow, App, App) {
    let (flow, mut old) = idle(test);
    connected(&flow, &mut old);
    flow.scratch.write("there", there.as_bytes());
    let before = sockets();
    old.run_command("oso-restart", None)
        .expect("oso-restart runs");
    let mut new = App::started_by_a_restart(&before);
    assert!(old.wait_for_exit().success(), "the instance going exits");
    new.wait_until_ready();
    (flow, old, new)
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_connected_session_comes_back_to_the_same_session_after_oso_restart() {
    let (flow, _old, mut new) = restarted("native-remote-restart", "cse_a1");
    let asked: Value = serde_json::from_str(&flow.wait_for_file("on.3")).unwrap();
    assert_eq!(asked["request"]["reattach_session_id"], "cse_a1");
    assert!(!there(&flow, "off.2"), "it was never told that it is off");
    assert!(!remote_control_off(&args(&flow, "run.3")));
    transcript_shown_again(&mut new);
    transcript_says(&mut new, "Remote Control was on before oso-restart");
    new.wait_for("the mark", |state| modeline(state).contains("⇄ claude.ai"));
    let _ = new.exchange("exit", None);
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn another_session_answered_after_oso_restart_turns_it_off() {
    let (flow, _old, mut new) = restarted("native-remote-restart-other", "cse_other");
    flow.wait_for_file("on.3");
    flow.wait_for_file("off.3");
    assert!(remote_control_off(&args(&flow, "run.4")));
    transcript_shown_again(&mut new);
    transcript_says(&mut new, "answered another session than this one's");
    new.wait_for("no mark", |state| !modeline(state).contains('⇄'));
    let _ = new.exchange("exit", None);
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_directory_made_again_before_oso_restart_connects_nothing() {
    let (flow, mut old) = idle("native-remote-restart-moved");
    connected(&flow, &mut old);
    // The repository is another at the same path.
    let git = flow.project.join(".git");
    fs::rename(&git, flow.scratch.path().join("git-before")).unwrap();
    git_init(&flow.project);
    let before = sockets();
    old.run_command("oso-restart", None)
        .expect("oso-restart runs");
    let mut new = App::started_by_a_restart(&before);
    assert!(old.wait_for_exit().success(), "the instance going exits");
    new.wait_until_ready();
    assert!(remote_control_off(&args(&flow, "run.3")));
    transcript_shown_again(&mut new);
    transcript_says(&mut new, "Remote Control stays off");
    assert!(!there(&flow, "on.3"));
    let _ = new.exchange("exit", None);
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn off_pressed_while_oso_restart_waits_is_not_connected_again() {
    let (flow, mut old) = idle("native-remote-restart-off");
    connected(&flow, &mut old);
    flow.scratch.write("slow", b"x");
    let before = sockets();
    old.run_command("oso-restart", None)
        .expect("oso-restart runs");
    old.run_command("native-remote-control", None).unwrap();
    let mut new = App::started_by_a_restart(&before);
    assert!(old.wait_for_exit().success(), "the instance going exits");
    new.wait_until_ready();
    assert!(remote_control_off(&args(&flow, "run.3")));
    thread::sleep(Duration::from_millis(500));
    assert!(!there(&flow, "on.3"));
    let _ = new.exchange("exit", None);
}

/// A flow whose person's Claude Code settings connect each session at start (ADR 0188), with
/// its session started and idle after one turn; and the transcript then.
fn at_startup(test: &str, remote: Option<&str>) -> (Flow, App) {
    let flow = Flow::with(test, REMOTE_FAKE, "");
    flow.scratch.write("there", b"cse_a1");
    let claude = flow.scratch.path().join("home/.claude");
    fs::create_dir_all(&claude).unwrap();
    fs::write(
        claude.join("settings.json"),
        r#"{"remoteControlAtStartup": true}"#,
    )
    .unwrap();
    if let Some(url) = remote {
        let config = flow.project.join(".git/config");
        let mut text = fs::read_to_string(&config).unwrap();
        text.push_str(&format!("[remote \"origin\"]\n\turl = {url}\n"));
        fs::write(&config, text).unwrap();
    }
    let mut app = flow.start();
    app.open_file(&flow.project.join("notes.md"));
    trust_restricted(&mut app, "proj");
    app.wait_for("the project trusted", |state| {
        !modeline(state).contains("restricted")
    });
    app.run_command("claude", None).unwrap();
    app.wait_for("the composer", in_composer);
    send(&mut app, "one");
    assert_eq!(flow.wait_for_file("sent.1.1").trim(), "one");
    transcript_says(&mut app, "reply 1.1");
    (flow, app)
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn claude_codes_setting_connects_each_start_until_the_person_turns_it_off() {
    let (flow, mut app) = at_startup("native-remote-at-startup", None);
    // Unblocked from the first run, asked by oso alone, and said so.
    let run = args(&flow, "run.1");
    assert!(!remote_control_off(&run), "{run:?}");
    let settings = run.iter().find(|arg| arg.starts_with("--settings="));
    assert!(
        settings.is_some_and(|s| s.contains(r#""remoteControlAtStartup":false"#)),
        "{run:?}"
    );
    let asked: Value = serde_json::from_str(&flow.wait_for_file("on.1")).unwrap();
    assert_eq!(asked["request"]["keep_session_on_exit"], true);
    transcript_says(
        &mut app,
        "as remoteControlAtStartup in your Claude Code settings says",
    );
    app.wait_for("the mark", |state| modeline(state).contains("⇄ claude.ai"));
    // The person's key turns it off, and the setting does not connect it again.
    app.run_command("native-remote-control", None).unwrap();
    flow.wait_for_file("off.1");
    assert!(remote_control_off(&args(&flow, "run.2")));
    transcript_says(&mut app, "Remote Control is off");
    app.wait_for("no mark", |state| !modeline(state).contains('⇄'));
    thread::sleep(Duration::from_millis(300));
    assert!(!there(&flow, "on.2"), "a blocked process is never asked");
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn claude_codes_setting_never_connects_a_repository_whose_remote_holds_a_password() {
    let (flow, mut app) = at_startup(
        "native-remote-at-startup-password",
        Some("https://me:secret@example.invalid/r"),
    );
    assert!(remote_control_off(&args(&flow, "run.1")));
    transcript_says(
        &mut app,
        "Remote Control stays off: a git remote's address holds a password.",
    );
    assert!(!there(&flow, "on.1"));
    let state = app.state();
    assert!(
        !state.to_string().contains("secret@"),
        "no address is shown"
    );
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}
