//! A native session's earlier turns (ADR 0165, part 5a), read from a transcript the flow
//! writes where Claude Code keeps it under the flow's home: a fork shows the turns of the
//! conversation it copies, then says it was forked; a session the new instance resumes after
//! `oso-restart` shows its turns before saying so; one with no transcript says why.
//!
//! Must not depend on: the app's internals; which lines are shown, and how, have their own
//! tests.

use std::fs;
use std::path::PathBuf;

use super::super::super::harness::{App, sockets};
use super::restart::{idle_in_accept_edits, session_id, show_transcript, transcript_says};
use super::resume::spoken_and_stopped;
use super::{Flow, in_composer, text};

/// The recorded transcript, scrubbed, run in `/w/proj`.
const RECORDED: &str =
    include_str!("../../../../src/sessions/native/fixtures/2.1.330/transcript.jsonl");

/// The recorded transcript written as session `id`'s, run in the flow's project as it
/// resolves, as the CLI writes it.
fn write_transcript(flow: &Flow, id: &str) -> PathBuf {
    let project = fs::canonicalize(&flow.project).unwrap();
    let slug: String = (project.to_string_lossy().chars())
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect();
    let dir = flow.scratch.path().join("home/.claude/projects").join(slug);
    fs::create_dir_all(&dir).unwrap();
    let cwd = serde_json::to_string(&project.to_string_lossy()).unwrap();
    let path = dir.join(format!("{id}.jsonl"));
    fs::write(&path, RECORDED.replace(r#""/w/proj""#, &cwd)).unwrap();
    path
}

/// Where `row` is in `shown`, which says it.
fn at(shown: &str, row: &str) -> usize {
    shown
        .find(row)
        .unwrap_or_else(|| panic!("no {row:?} in {shown}"))
}

/// The session id run 1 started with, from its arguments.
fn first_id(flow: &Flow) -> String {
    let run = flow.wait_for_file("run.1");
    run.lines()
        .find_map(|line| line.strip_prefix("arg --session-id="))
        .expect("a session id")
        .to_owned()
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_native_fork_shows_the_turns_it_copies() {
    let flow = Flow::new("native-past-fork");
    let mut app = spoken_and_stopped(&flow);
    write_transcript(&flow, &first_id(&flow));
    app.run_command("session-fork", None).unwrap();
    let args = flow.wait_for_file("run.2");
    assert!(args.contains("arg --fork-session"), "{args}");
    app.wait_for("the fork's composer", in_composer);
    app.send_keys("C-c C-o");
    let state = app.wait_for("the fork's transcript", |state| {
        state["buffer"]["name"] != "*claude: proj*" && text(state).contains("Forked from")
    });
    let shown = text(&state);
    let prompt = at(&shown, "Reply with the single word apple.");
    assert!(prompt < at(&shown, "note.txt"), "{shown}");
    assert!(
        at(&shown, "plum") < at(&shown, "Forked from proj."),
        "{shown}"
    );
    assert!(!shown.contains("did not read"), "{shown}");
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_session_resumed_after_a_restart_shows_its_turns_first() {
    let (flow, mut old) = idle_in_accept_edits("native-past-restart", None);
    let id = session_id(&flow);
    write_transcript(&flow, &id);
    let before = sockets();
    old.run_command("oso-restart", None)
        .expect("oso-restart runs");
    let mut new = App::started_by_a_restart(&before);
    assert!(old.wait_for_exit().success(), "the instance going exits");
    new.wait_until_ready();
    flow.wait_for_file("run.2");
    show_transcript(&mut new);
    transcript_says(&mut new, "Resumed after oso-restart.");
    let state = new.wait_for("the earlier turns", |state| {
        text(state).contains("Reply with the single word apple.")
    });
    let shown = text(&state);
    let resumed = at(&shown, "Resumed after oso-restart.");
    assert!(at(&shown, "plum") < resumed, "{shown}");
    assert!(at(&shown, "Stopped.") < at(&shown, "apple"), "{shown}");
    let _ = new.exchange("exit", None);
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_fork_with_no_transcript_says_why_and_still_starts() {
    let flow = Flow::new("native-past-none");
    let mut app = spoken_and_stopped(&flow);
    app.run_command("session-fork", None).unwrap();
    flow.wait_for_file("run.2");
    app.wait_for("the fork's composer", in_composer);
    app.send_keys("C-c C-o");
    let state = app.wait_for("the fork's transcript", |state| {
        state["buffer"]["name"] != "*claude: proj*" && text(state).contains("Forked from")
    });
    let shown = text(&state);
    let why = at(&shown, "its transcript was not found");
    assert!(why < at(&shown, "Forked from proj."), "{shown}");
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}
