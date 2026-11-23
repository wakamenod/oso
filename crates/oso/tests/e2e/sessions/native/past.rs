//! A native session's earlier turns (ADR 0165, part 5a), read from a transcript the flow
//! writes where Claude Code keeps it under the flow's home: a fork shows the turns of the
//! conversation it copies, then says it was forked; a session the new instance resumes after
//! `oso-restart` shows its turns before saying so; one with no transcript says why; and a
//! session back from the TUI shows the TUI's turns after its own rows (part 5b).
//!
//! Must not depend on: the app's internals; which lines are shown, and how, have their own
//! tests.

use std::fs;
use std::path::PathBuf;

use serde_json::json;

use super::super::super::harness::{App, sockets};
use super::restart::{idle_in_accept_edits, session_id, show_transcript, transcript_says};
use super::resume::spoken_and_stopped;
use super::switch::{in_the_tui, switch_back, taken_back};
use super::{Flow, in_composer, text};

/// The recorded transcript, scrubbed, run in `/w/proj`.
const RECORDED: &str =
    include_str!("../../../../src/sessions/native/fixtures/2.1.330/transcript.jsonl");

/// The recorded transcript written as session `id`'s, run in the flow's project as it
/// resolves, as the CLI writes it.
fn write_transcript(flow: &Flow, id: &str) -> PathBuf {
    write_lines(flow, id, RECORDED)
}

/// `lines`, run in `/w/proj`, written as session `id`'s transcript, run in the flow's project.
fn write_lines(flow: &Flow, id: &str, lines: &str) -> PathBuf {
    let project = fs::canonicalize(&flow.project).unwrap();
    let slug: String = (project.to_string_lossy().chars())
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect();
    let dir = flow.scratch.path().join("home/.claude/projects").join(slug);
    fs::create_dir_all(&dir).unwrap();
    let cwd = serde_json::to_string(&project.to_string_lossy()).unwrap();
    let path = dir.join(format!("{id}.jsonl"));
    fs::write(&path, lines.replace(r#""/w/proj""#, &cwd)).unwrap();
    path
}

/// The transcript in front, whole: its first turns may be above what the window shows.
fn whole(app: &mut App) -> String {
    let text = app.call("get-text", Some(json!({ "start_row": 0, "rows": 1000 })));
    let rows = text["rows"].as_array().expect("rows");
    let rows: Vec<&str> = rows.iter().filter_map(|row| row.as_str()).collect();
    rows.join("\n")
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
    app.wait_for("the fork's transcript", |state| {
        state["buffer"]["name"] != "*claude: proj*" && text(state).contains("Forked from")
    });
    let shown = whole(&mut app);
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
    // Its last turn, then the rest above it.
    new.wait_for("the earlier turns", |state| text(state).contains("plum"));
    let shown = whole(&mut new);
    at(&shown, "Reply with the single word apple.");
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
    // The note wraps at the window's width.
    let why = at(&shown, "Its earlier turns are not shown");
    assert!(why < at(&shown, "Forked from proj."), "{shown}");
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}

/// What the switch fake's first reply and then a turn in the TUI write: the native reply's
/// line `a11`, then the person's message typed in the TUI and its reply.
const AFTER_THE_TUI: &str = r#"{"parentUuid":null,"promptSource":"sdk","type":"user","cwd":"/w/proj","message":{"role":"user","content":"hello there"},"uuid":"u11"}
{"parentUuid":"u11","type":"assistant","cwd":"/w/proj","message":{"id":"m11","role":"assistant","content":[{"type":"text","text":"reply 1.1"}]},"uuid":"a11"}
{"parentUuid":"a11","promptSource":"typed","origin":{"kind":"human"},"type":"user","cwd":"/w/proj","message":{"role":"user","content":"typed in the TUI"},"uuid":"u21"}
{"parentUuid":"u21","type":"assistant","cwd":"/w/proj","message":{"id":"m21","role":"assistant","content":[{"type":"text","text":"answered in the TUI"}]},"uuid":"a21"}
"#;

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_session_back_from_the_tui_shows_the_tuis_turns_after_its_rows() {
    let flow = super::switch::flow("native-past-back");
    let (mut app, id) = in_the_tui(&flow);
    write_lines(&flow, &id, AFTER_THE_TUI);
    switch_back(&mut app);
    taken_back(&mut app);
    let state = app.wait_for("the TUI's turns", |state| {
        text(state).contains("answered in the TUI") && text(state).contains("Resumed.")
    });
    let shown = text(&state);
    // Its own rows once, then the TUI's, then the resume.
    assert_eq!(shown.matches("reply 1.1").count(), 1, "{shown}");
    let back = at(&shown, "Back from the TUI");
    assert!(at(&shown, "reply 1.1") < back, "{shown}");
    assert!(back < at(&shown, "Its turns since"), "{shown}");
    assert!(
        at(&shown, "typed in the TUI") < at(&shown, "answered in the TUI"),
        "{shown}"
    );
    assert!(
        at(&shown, "answered in the TUI") < at(&shown, "Resumed."),
        "{shown}"
    );
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}
