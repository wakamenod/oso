//! Text oso composed, in a native session (ADR 0165, step 2f), with the composer's fake
//! `claude -p`: `session-send`'s note goes into the composer at point, after the person's
//! draft, not sent, its leading `/` and word-initial `@` escaped, and goes as written once the
//! person sends it; a review's question the person confirmed goes at once, escaped, the draft
//! left as it was.
//!
//! Must not depend on: the app's internals; the escape has its own tests.

use std::ffi::OsStr;
use std::fs;
use std::path::Path;

use oso_platform::process::{Executable, Sink, Source, Spawn, spawn};

use super::super::super::harness::App;
use super::super::super::spaces::trust_restricted;
use super::super::super::worktrees::{asks, keys};
use super::compose::COMPOSE_FAKE;
use super::{Flow, MESSAGE, end_turn, in_composer, text};

/// Runs git in `dir` with `args`, which must succeed.
fn git(dir: &Path, args: &[&str]) {
    let git = Executable::from_path("git", None).expect("git is on PATH");
    let args: Vec<std::ffi::OsString> = args.iter().map(Into::into).collect();
    let child = spawn(&Spawn {
        executable: &git,
        args: &args,
        cwd: Some(dir),
        project: None,
        inherit: &[],
        stdin: Source::Closed,
        stdout: Sink::Null,
        stderr: Sink::Null,
        own_process_group: false,
        own_session: false,
        set: &[
            ("GIT_CONFIG_GLOBAL", OsStr::new("/dev/null")),
            ("GIT_AUTHOR_NAME", OsStr::new("e2e")),
            ("GIT_AUTHOR_EMAIL", OsStr::new("e2e@example.invalid")),
            ("GIT_COMMITTER_NAME", OsStr::new("e2e")),
            ("GIT_COMMITTER_EMAIL", OsStr::new("e2e@example.invalid")),
        ],
        terminal: None,
        pass_fd: None,
        args_logged: true,
    })
    .unwrap();
    assert!(
        child.wait_with_output().unwrap().status.success(),
        "{args:?}"
    );
}

/// The composer's text, once it is selected.
fn composer_text(app: &mut App) -> String {
    let state = app.wait_for("the composer", in_composer);
    text(&state)
}

/// Leaves the composer's dock for the transcript, then opens `notes.md` there and waits until
/// it shows.
fn open_notes(flow: &Flow, app: &mut App) {
    app.send_keys("C-c C-o");
    app.wait_for("the transcript", |state| {
        state["buffer"]["name"] == "*claude: proj*"
    });
    app.open_file(&flow.project.join("notes.md"));
    app.wait_for("notes.md", |state| {
        state["buffer"]["name"] == "notes.md" && state["buffer"]["loading"].is_null()
    });
}

/// What the echo area says.
fn echo(state: &serde_json::Value) -> String {
    state["echo"].as_str().unwrap_or_default().to_owned()
}

/// The flow, with the composer's fake and a commit of `notes.md`, its session started in the
/// trusted project and its first message sent and answered.
fn started() -> (Flow, App) {
    let flow = Flow::new("native-composed");
    let script = COMPOSE_FAKE.replace("DIR", &flow.scratch.path().display().to_string());
    fs::write(flow.scratch.path().join("fake-claude"), script).unwrap();
    let init = r#"{"type":"system","subtype":"init","cwd":"/w","session_id":"s","model":"m"}"#;
    flow.scratch
        .write("stream.jsonl", format!("{init}\n").as_bytes());
    git(&flow.project, &["add", "notes.md"]);
    git(
        &flow.project,
        &["commit", "-q", "--no-gpg-sign", "-m", "notes"],
    );
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
    assert_eq!(flow.wait_for_file("sent.1").trim(), MESSAGE);
    flow.scratch.write("release.1", b"");
    (flow, app)
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_send_goes_into_the_composer_escaped_and_a_reviews_question_goes_at_once() {
    let (flow, mut app) = started();

    // A draft, then a note sent from another buffer: it goes in after the draft, escaped, and
    // nothing is sent.
    app.send_keys(&keys("draft"));
    open_notes(&flow, &mut app);
    app.send_keys("C-g C-c a m");
    app.wait_for("the session prompt", |state| {
        asks(state).is_some_and(|prompt| prompt.starts_with("Send to session"))
    });
    app.send_keys(&format!("{} RET", keys("*claude: proj*")));
    app.wait_for("the note prompt", |state| {
        asks(state).is_some_and(|prompt| prompt.starts_with("Note"))
    });
    app.send_keys(&format!("{} RET", keys("/look at @notes.md")));
    let state = app.wait_for("what the send said", |state| {
        asks(state).is_none() && echo(state).starts_with("Put into")
    });
    assert_eq!(
        echo(&state),
        "Put into proj's composer: C-c C-c in its composer sends it"
    );
    assert_eq!(composer_text(&mut app), "draft /look at \\@notes.md");
    assert!(
        !flow.scratch.path().join("sent.2").exists(),
        "a send is not sent"
    );
    // The person sends it as it reads.
    app.send_keys("C-c C-c");
    assert_eq!(
        flow.wait_for_file("sent.2").trim(),
        r"draft /look at \\@notes.md"
    );
    // Its turn ends before the question, which would wait in the queue while it runs.
    end_turn(&flow, &mut app, "release.2");

    // A draft stays while a review's question goes at once, escaped.
    app.send_keys(&keys("kept"));
    fs::write(flow.project.join("notes.md"), "notes\nmore\n").unwrap();
    // From a buffer of the project, whose uncommitted changes the review shows.
    open_notes(&flow, &mut app);
    app.send_keys("C-u C-c a v w");
    app.wait_for("the review", |state| {
        state["buffer"]["name"] == "*review: uncommitted proj*" && text(state).contains("+more")
    });
    app.send_keys("M-< n C-n Q");
    app.wait_for("the question's prompt", |state| {
        asks(state).is_some_and(|prompt| prompt.starts_with("Ask about this"))
    });
    app.send_keys(&format!("{} RET", keys("Why @notes.md")));
    let state = app.wait_for("the question asked", |state| {
        echo(state).starts_with("Asked ")
    });
    assert!(
        echo(&state).starts_with("Asked proj, sent"),
        "{}",
        echo(&state)
    );
    let sent = flow.wait_for_file("sent.3");
    assert!(sent.contains(r"Question: Why \\@notes.md"), "{sent}");
    flow.scratch.write("release.3", b"");
    app.send_keys("C-x b");
    app.send_keys(&format!("{} RET", keys("*claude: proj*")));
    app.send_keys("i");
    assert_eq!(composer_text(&mut app), "kept");
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}
