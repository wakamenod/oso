//! A native session across `oso-restart` (ADR 0165, part 4c), with the modes' fake `claude -p`,
//! whose run `N` writes its arguments to `run.N` and ends once its input does: the instance
//! going ends the idle session's process, and the new instance resumes it, with `--resume` and
//! the mode the person chose; a restart that stays (a dev build's `OSO_DEV_RESTART_FAIL`),
//! before the new instance took the state or after, resumes it in the instance going instead,
//! and the new instance resumes nothing.
//!
//! Must not depend on: the app's internals; the state's field, the order's signals and the
//! mode carried have their own tests.

use std::ffi::OsStr;
use std::fs;
use std::time::{Duration, Instant};

use super::super::super::harness::{App, sockets};
use super::super::super::spaces::trust_restricted;
use super::super::super::worktrees::keys;
use super::modes::{MODES_FAKE, footer_says, modeline, send};
use super::{Flow, in_composer, text};

/// A flow on tier 2 whose session switched to `acceptEdits` and ended a turn, idle, with the
/// composer selected. Its instance's restart stops short at `fail`, if any.
pub(super) fn idle_in_accept_edits(test: &str, fail: Option<&str>) -> (Flow, App) {
    let flow = Flow::with(test, MODES_FAKE, "");
    flow.scratch.write("tier", b"2");
    // The fake says the new mode at once.
    flow.scratch.write("say.acceptEdits", b"x");
    let home = flow.scratch.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let mut set = vec![("HOME", home.as_os_str())];
    if let Some(fail) = fail {
        set.push(("OSO_DEV_RESTART_FAIL", OsStr::new(fail)));
    }
    let mut app = App::start_with_config_and_env(&flow.project, &flow.config, &set);
    app.wait_until_ready();
    app.open_file(&flow.project.join("notes.md"));
    trust_restricted(&mut app, "proj");
    app.wait_for("the project trusted", |state| {
        !modeline(state).contains("restricted")
    });
    app.run_command("claude", None).unwrap();
    app.wait_for("the composer", in_composer);
    send(&mut app, "one");
    assert_eq!(flow.wait_for_file("sent.1.1").trim(), "one");
    app.send_keys("S-TAB");
    assert_eq!(flow.wait_for_file("asked.1").trim(), "acceptEdits");
    footer_says(&mut app, "▶▶ accept edits on");
    (flow, app)
}

/// The session id run 1 started with, `--session-id=ID`.
pub(super) fn session_id(flow: &Flow) -> String {
    let run = flow.wait_for_file("run.1");
    run.lines()
        .find_map(|line| line.strip_prefix("arg --session-id="))
        .expect("a session id")
        .to_owned()
}

/// Run 2's arguments: a resume of `id` in `acceptEdits`.
fn resumed_in_accept_edits(flow: &Flow, id: &str) {
    let run = flow.wait_for_file("run.2");
    assert!(run.contains(&format!("arg --resume={id}")), "{run}");
    assert!(run.contains("arg --permission-mode=acceptEdits"), "{run}");
}

/// Shows the session's transcript, which the new instance does not show at first, as it
/// comes back behind the file.
pub(super) fn show_transcript(app: &mut App) {
    app.send_keys("C-x b");
    app.send_keys(&keys("*claude: proj*"));
    app.send_keys("RET");
}

/// Waits until `app`'s transcript says `row`.
pub(super) fn transcript_says(app: &mut App, row: &str) {
    app.send_keys("C-c C-o");
    app.wait_for(row, |state| {
        state["buffer"]["name"] == "*claude: proj*" && text(state).contains(row)
    });
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_native_session_resumes_in_the_instance_oso_restart_starts_in_its_mode() {
    let (flow, mut old) = idle_in_accept_edits("native-restart-resume", None);
    let id = session_id(&flow);
    let before = sockets();
    old.run_command("oso-restart", None)
        .expect("oso-restart runs");
    let mut new = App::started_by_a_restart(&before);
    assert!(old.wait_for_exit().success(), "the instance going exits");
    new.wait_until_ready();
    // Run 1 ended with its input, and only then, the old instance gone, did run 2 start.
    resumed_in_accept_edits(&flow, &id);
    show_transcript(&mut new);
    transcript_says(&mut new, "Resumed after oso-restart.");
    new.wait_for("the mode", |state| modeline(state).contains("acceptEdits"));
    // The resumed session takes messages: run 2's first.
    new.send_keys("i");
    new.wait_for("the composer", in_composer);
    send(&mut new, "two");
    assert_eq!(flow.wait_for_file("sent.2.1").trim(), "two");
    let _ = new.exchange("exit", None);
}

/// Waits until `app`'s restart has stayed, and its session resumed there. The echo's
/// "everything stays here" may give way at once to the agent channel's coming back on.
fn stayed_and_resumed(flow: &Flow, app: &mut App, id: &str) {
    resumed_in_accept_edits(flow, id);
    transcript_says(app, "oso-restart stayed here: resumed.");
    app.send_keys("i");
    app.wait_for("the composer", in_composer);
    send(app, "two");
    assert_eq!(flow.wait_for_file("sent.2.1").trim(), "two");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_restart_that_stays_resumes_the_native_session_in_the_instance_going() {
    let (flow, mut app) = idle_in_accept_edits("native-restart-stays", Some("hello"));
    let id = session_id(&flow);
    app.run_command("oso-restart", None)
        .expect("oso-restart runs");
    stayed_and_resumed(&flow, &mut app, &id);
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_new_instance_told_to_stay_out_resumes_nothing() {
    let (flow, mut old) = idle_in_accept_edits("native-restart-stay-out", Some("exit"));
    let id = session_id(&flow);
    let before = sockets();
    old.run_command("oso-restart", None)
        .expect("oso-restart runs");
    let mut new = App::started_by_a_restart(&before);
    stayed_and_resumed(&flow, &mut old, &id);
    // The new instance closes, as nothing changed in it, and starts no run of its own.
    let since = Instant::now();
    while !new.exited() {
        assert!(since.elapsed() < Duration::from_secs(20), "it stays");
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        !flow.scratch.path().join("run.3").exists(),
        "a second writer"
    );
    let _ = old.exchange("exit", None);
    old.wait_for_exit();
}
