//! Native sessions an agent started in worktrees, with no file of the worktree open: resumed
//! (ADR 0165, "Worktrees' resumes"), and what their transcripts count for ("Transcripts count
//! as open"), with a fake `claude -p` that answers each message and runs
//! its bridge as `session_tools`' fake does. `oso-restart` resumes one whose repository is
//! trusted, and it takes a message; one whose repository was untrusted meanwhile stays stopped
//! and runs nothing, its transcript, its row of `*Spaces*`, the echo area and `*Messages*`
//! saying so, until it is trusted again and `session-resume` resumes it; one whose turn ends with
//! changes is ready for review; and an orchestrator's `session_resume` resumes a stopped one
//! without asking, and asks once its repository is untrusted. One started with a model is
//! resumed with it (ADR 0121, "A start's model, as amended").
//!
//! Must not depend on: the app's internals; each refusal of the claim has its own test. Driven
//! by its control socket and the fake's files alone; no real key event is posted.

use std::fs;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::harness::{App, sockets};
use super::session_tools::{Bridge, Run, answer, run_in, runs};
use super::spaces::{PrivateRoot, in_project, trust_restricted};
use super::worktrees::{DEADLINE, Flow, asks, keys, text_until};

/// The fake: where it runs and its arguments, as `session_tools`' fake writes them, and the
/// bridge its flow asks for, run as its child. Each message `M` it writes to `sent.PID.M`, and
/// answers; one that says `exit then` ends it too.
const FAKE: &str = r#"#!/bin/sh
dir='DIR'
printf '%s\0' "$(pwd -P)" "$@" > "$dir/tmp.$$"
mkfifo "$dir/in.$$"
mv "$dir/tmp.$$" "$dir/run.$$"
(
  while [ ! -f "$dir/bridge.$$" ]; do
    [ -d "$dir" ] || exit 0
    sleep 0.1
  done
  exec sh "$dir/bridge.$$" < "$dir/in.$$" > "$dir/out.$$" 2> "$dir/err.$$"
) < /dev/null > /dev/null 2>&1 &
m=0
while IFS= read -r line; do
  case "$line" in *'"control_request"'*) continue ;; esac
  m=$((m+1))
  said=$(printf '%s\n' "$line" | sed 's/.*"content":"\([^"]*\)".*/\1/')
  printf '%s\n' "$said" > "$dir/sent.$$.$m"
  printf '{"type":"system","subtype":"init","cwd":"/w","session_id":"s","permissionMode":"default","capabilities":[]}\n'
  printf '{"type":"assistant","message":{"id":"m%s","content":[{"type":"text","text":"reply %s"}]}}\n' "$m" "$m"
  printf '{"type":"result","subtype":"success","duration_ms":10}\n'
  case "$said" in *'exit then'*) exit 0 ;; esac
done
"#;

const WORKTREE: &str = "*claude: e2e-own*";

/// What the instance a restart started resumes, its echo area and its sessions.
fn shown(state: &Value) -> String {
    format!(
        "buffer {}; echo {}; sessions {}",
        state["buffer"], state["echo"], state["sessions"]
    )
}

/// `app`'s state once `done`, else a failure with what it shows.
fn until(app: &mut App, what: &str, done: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let state = app.state();
        if done(&state) {
            return state;
        }
        assert!(Instant::now() < deadline, "no {what}; {}", shown(&state));
        thread::sleep(Duration::from_millis(100));
    }
}

/// A flow of the fake's, with the sessions' mode left to its default, native.
fn native(test: &str, sessions: &str) -> Flow {
    let flow = Flow::with_fake(test, sessions, FAKE);
    let config = flow.scratch.path().join("config.toml");
    let text = fs::read_to_string(&config).unwrap();
    flow.scratch.write(
        "config.toml",
        text.replace("mode = \"tui\"\n", "").as_bytes(),
    );
    flow
}

/// The files the flow's runs wrote whose names start with `prefix`.
fn written(flow: &Flow, prefix: &str) -> Vec<String> {
    fs::read_dir(flow.scratch.path())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(prefix))
        .filter_map(|entry| fs::read_to_string(entry.path()).ok())
        .collect()
}

/// Waits until run `run` was given `message`.
fn sent(flow: &Flow, run: &Run, message: &str) {
    let deadline = Instant::now() + DEADLINE;
    while !(written(flow, &format!("sent.{}.", run.pid)).iter()).any(|said| said.contains(message))
    {
        assert!(Instant::now() < deadline, "{message:?} was not sent");
        thread::sleep(Duration::from_millis(100));
    }
}

/// The run that resumed conversation `id`, once it is there.
fn resumed_run(flow: &Flow, app: &mut App, id: &str) -> Run {
    let resume = format!("--resume={id}");
    let find = || {
        runs(flow)
            .into_iter()
            .find(|run| run.args.contains(&resume))
    };
    until(app, "run that resumed", |_| find().is_some());
    find().unwrap()
}

/// An instance with a native session an agent started in a worktree, which answered its brief,
/// and no file of the worktree open; the session's id and worktree.
fn in_a_worktree(flow: &Flow) -> (App, String, PathBuf) {
    let (app, id, worktree, _) = in_a_worktree_with(flow, None);
    (app, id, worktree)
}

/// As [`in_a_worktree`], the session started with `model`, if given; and its first run and the
/// question asked.
fn in_a_worktree_with(flow: &Flow, model: Option<&str>) -> (App, String, PathBuf, (Run, String)) {
    let (mut app, _) = flow.start();
    app.run_command("claude", None).unwrap();
    let run = run_in(flow, &flow.repo);
    let mut bridge = Bridge::start(flow, &run);
    let mut call = json!({ "branch": "e2e-own", "brief": "Fix the parser" });
    if let Some(model) = model {
        call["model"] = json!(model);
    }
    let id = bridge.call("start_worktree_session", call);
    let question = answer(&mut app, &bridge, "y");
    let (started, error) = bridge.result(id);
    assert!(!error, "{started}");
    let worktree = flow.repo.join(".claude/worktrees/e2e-own");
    let run = run_in(flow, &worktree);
    sent(flow, &run, "Fix the parser");
    let id = run
        .args
        .iter()
        .find_map(|a| a.strip_prefix("--session-id="));
    let id = id.expect("a session id").to_owned();
    // Its turn ended, so that the restart does not wait for it.
    until(&mut app, "idle worktree session", |state| {
        settled(state, "e2e-own")
    });
    (app, id, worktree, (run, question))
}

/// Session `name` as the dev state lists it.
fn listed<'a>(state: &'a Value, name: &str) -> Option<&'a Value> {
    (state["sessions"]["sessions"].as_array()?)
        .iter()
        .find(|session| session["name"] == name)
}

/// Whether session `name`'s turn ended, seen or not.
fn settled(state: &Value, name: &str) -> bool {
    listed(state, name)
        .is_some_and(|session| session["state"] == "idle" || session["state"] == "done")
}

/// `old`'s `oso-restart`: the instance it starts, which writes its log where `old` did, so `old`
/// is kept as long.
fn restarted(old: &mut App) -> App {
    let before = sockets();
    old.run_command("oso-restart", None)
        .expect("oso-restart runs");
    let mut new = App::started_by_a_restart(&before);
    assert!(old.wait_for_exit().success(), "the instance going exits");
    new.wait_until_ready();
    new
}

/// Selects buffer `name`.
fn select(app: &mut App, name: &str) {
    app.send_keys("C-x b");
    app.send_keys(&keys(name));
    app.send_keys("RET");
    until(app, name, |state| state["buffer"]["name"] == name);
}

/// Waits until the worktree session's transcript says `row`, wrapped anywhere.
fn says(app: &mut App, row: &str) {
    let bare = |text: &str| text.split_whitespace().collect::<String>();
    let row_bare = bare(row);
    until(app, row, |state| {
        let rows = state["text"]["rows"].as_array().into_iter().flatten();
        let shown: String = rows.filter_map(Value::as_str).map(bare).collect();
        state["buffer"]["name"] == WORKTREE && shown.contains(&row_bare)
    });
}

/// Sends the worktree session, whose transcript is selected, `message` by its composer.
fn send(app: &mut App, message: &str) {
    app.send_keys("i");
    until(app, "composer", |state| state["buffer"]["name"] != WORKTREE);
    app.send_keys(&keys(message));
    app.send_keys("C-c C-c");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_turn_that_ends_with_changes_is_ready_for_review_with_no_file_open() {
    let flow = native("worktree-review", "");
    let (mut app, _, worktree) = in_a_worktree(&flow);
    fs::write(worktree.join("work.txt"), "work\n").unwrap();
    select(&mut app, WORKTREE);
    send(&mut app, "two");
    sent(&flow, &run_in(&flow, &worktree), "two");
    until(&mut app, "session ready for review", |state| {
        listed(state, "e2e-own").is_some_and(|session| session["state"] == "ready for review")
    });
    let _ = app.exchange("exit", None);
}

/// The `--model` arguments of `run`.
fn models(run: &Run) -> Vec<&str> {
    (run.args.iter())
        .filter(|arg| arg.starts_with("--model"))
        .map(String::as_str)
        .collect()
}

/// A native worktree session an agent started with a model runs `claude -p --model=MODEL`,
/// the question names it, and `oso-restart` resumes it with the model again, its own, which
/// the record kept (ADR 0121, "A start's model, as amended").
#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_native_session_started_with_a_model_keeps_it_over_oso_restart() {
    let flow = native("worktree-resume-model", "");
    let (mut old, id, _, (run, question)) = in_a_worktree_with(&flow, Some("haiku"));
    assert!(question.contains("Model: haiku."), "{question}");
    assert!(run.args.iter().any(|arg| arg == "-p"), "{:?}", run.args);
    assert_eq!(models(&run), ["--model=haiku"], "{:?}", run.args);
    let mut new = restarted(&mut old);
    let resumed = resumed_run(&flow, &mut new, &id);
    assert_eq!(models(&resumed), ["--model=haiku"], "{:?}", resumed.args);
    let _ = new.exchange("exit", None);
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn oso_restart_resumes_a_worktree_session_with_no_file_of_it_open() {
    let flow = native("worktree-resume", "");
    let (mut old, id, worktree) = in_a_worktree(&flow);
    let mut new = restarted(&mut old);
    let run = resumed_run(&flow, &mut new, &id);
    assert_eq!(run.cwd, fs::canonicalize(&worktree).unwrap());
    select(&mut new, WORKTREE);
    says(&mut new, "Resumed after oso-restart.");
    let state = new.state();
    assert_ne!(listed(&state, "e2e-own").unwrap()["state"], "stopped");
    // It takes a message.
    send(&mut new, "two");
    sent(&flow, &run, "two");
    let _ = new.exchange("exit", None);
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn one_whose_repository_was_untrusted_stays_stopped_until_it_is_trusted_again() {
    const WHY: &str = "its repository is not trusted now: trust-project it, then session-resume";
    let flow = native("worktree-resume-untrusted", "");
    let (mut old, id, _) = in_a_worktree(&flow);
    select(&mut old, "README.md");
    old.run_command("untrust-project", None).unwrap();
    until(&mut old, "project restricted", |state| {
        (state["echo"].as_str()).is_some_and(|echo| echo.ends_with("is restricted again"))
    });
    let mut new = restarted(&mut old);
    // Said once every resume reported and the agent channel came on, if it does.
    let left = "oso-restart left 1 session stopped: e2e-own; its transcript says why";
    let state = until(&mut new, "echo of what was left stopped", |state| {
        state["echo"] == left
    });
    let session = listed(&state, "e2e-own").expect("its row");
    assert_eq!(session["state"], "stopped", "{}", shown(&state));
    let row = "not resumed: repository not trusted; trust-project, then session-resume";
    assert_eq!(session["not_resumed"], row, "{}", shown(&state));
    select(&mut new, "*Messages*");
    let lines = text_until(&mut new, "line of what was left stopped", |lines| {
        lines.iter().any(|line| line == left)
    });
    let said = |line: &str| line.starts_with("Agent channel") || line == left;
    let said: Vec<&String> = lines.iter().filter(|line| said(line)).collect();
    assert_eq!(said.last(), Some(&&left.to_owned()), "{lines:?}");
    assert_eq!(
        said.iter().filter(|line| **line == left).count(),
        1,
        "{lines:?}"
    );
    select(&mut new, WORKTREE);
    says(
        &mut new,
        &format!("It did not resume after oso-restart: {WHY}"),
    );
    let resume = format!("--resume={id}");
    assert!(
        !runs(&flow).iter().any(|run| run.args.contains(&resume)),
        "a second run"
    );
    // Trusted again, the person resumes it.
    select(&mut new, "README.md");
    trust_restricted(&mut new, "repo");
    // By the modeline: a message of oso's own may take the echo area.
    until(&mut new, "project trusted", |state| {
        in_project(state, "repo")
    });
    select(&mut new, WORKTREE);
    new.run_command("session-resume", None).unwrap();
    resumed_run(&flow, &mut new, &id);
    until(&mut new, "row that no longer says why", |state| {
        listed(state, "e2e-own").is_some_and(|session| session["not_resumed"].is_null())
    });
    let _ = new.exchange("exit", None);
}

/// A worker `name` the orchestrator behind `bridge` starts in a worktree, without asking, which
/// ended its first turn and stopped; its session's id and its worktree.
fn stopped_worker(
    flow: &Flow,
    app: &mut App,
    bridge: &mut Bridge,
    name: &str,
) -> (String, PathBuf) {
    let call = json!({ "branch": name, "brief": "exit then", "name": name, "repo": "repo" });
    let (started, error) = bridge.tool_when_free("start_worktree_session", &call);
    assert!(!error, "{started}");
    let id = started["session"]["id"].as_str().unwrap().to_owned();
    let worktree = flow.repo.join(".claude/worktrees").join(name);
    sent(flow, &run_in(flow, &worktree), "exit then");
    until(app, "stopped worker", |state| {
        listed(state, name).is_some_and(|session| session["state"] == "stopped")
    });
    (id, worktree)
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn an_orchestrators_session_resume_asks_only_once_the_repository_is_untrusted() {
    let spaces = PrivateRoot::new("worktree-resume-tool");
    let config = format!(
        "agents_start_without_asking = true\n\n[spaces]\nroot = \"{}\"\n",
        spaces.0.display()
    );
    let flow = native("worktree-resume-tool", &config);
    let (mut app, mut bridge, _) = super::space_tools::orchestrated(&flow, &spaces);
    let (id, worktree) = stopped_worker(&flow, &mut app, &mut bridge, "wa");
    stopped_worker(&flow, &mut app, &mut bridge, "wb");
    // No question: its transcript has its project open, and its project is trusted.
    let (resumed, error) = bridge.tool_when_free("session_resume", &json!({ "session": "wa" }));
    assert!(!error, "{resumed}");
    assert_eq!(asks(&app.state()), None);
    let run = resumed_run(&flow, &mut app, &id);
    assert_eq!(run.cwd, fs::canonicalize(&worktree).unwrap());
    // Untrusted now, the other one's asks.
    select(&mut app, "README.md");
    app.run_command("untrust-project", None).unwrap();
    until(&mut app, "project restricted", |state| {
        (state["echo"].as_str()).is_some_and(|echo| echo.ends_with("is restricted again"))
    });
    let call = bridge.call("session_resume", json!({ "session": "wb" }));
    let question = answer(&mut app, &bridge, "n");
    assert!(question.contains("resume session"), "{question}");
    let (refused, error) = bridge.result(call);
    assert!(error, "{refused}");
    let _ = app.exchange("exit", None);
}
