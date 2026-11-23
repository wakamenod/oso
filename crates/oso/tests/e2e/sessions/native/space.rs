//! A space's orchestrator as a native session in the space's own directory (ADR 0176): its
//! resume, a fork of it and its switches to the TUI and back run natively there under the
//! space's gate, reading the person's own settings alone (`--setting-sources=user`), and the
//! TUI's line reads the directory's as before; a session an agent started there is refused.
//! The space and its sessions are a record written before oso starts, in the flow's own state
//! directory, with the space's directory under a private root of the flow's.
//!
//! Must not depend on: the app's internals. Driven by its control socket and the fake's pty
//! alone; no real key event is posted.

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use super::super::super::harness::{App, emergency_dir};
use super::super::super::spaces::PrivateRoot;
use super::super::super::worktrees::keys;
use super::switch::{args, flag, flow, screen, switch_back};
use super::{Flow, in_composer, text};

/// The orchestrator's id, and that of a session an agent started in the space's directory.
const ORCHESTRATOR: &str = "00000001-0000-4000-8000-000000000000";
const AGENTS: &str = "00000002-0000-4000-8000-000000000000";

/// What the person's own settings alone are, on a native line.
const USER_ONLY: &str = "--setting-sources=user";

/// A record line of a stopped native session `name`, `id`, in `directory`, started by the
/// person or, with `parent`, by that session's agent.
fn session_line(id: &str, name: &str, directory: &Path, parent: Option<&str>) -> String {
    let directory = directory.display();
    let started = match parent {
        Some(parent) => format!(r#""origin":"answered","parent":"{parent}""#),
        None => r#""origin":"person""#.to_owned(),
    };
    format!(
        r#"{{"id":"{id}","name":"{name}","directory":"{directory}",{started},"pinned":false,"archived":false,"changed":1794512815000,"native":true}}"#
    )
}

/// Writes `name`, mode 0600, in `state`.
fn write_private(state: &Path, name: &str, text: &str) {
    let path = state.join(name);
    fs::write(&path, text).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
}

/// A switch flow whose config puts spaces under a private root of its own, and whose record
/// holds space `fix-login` of the flow's project, its directory made there, with its stopped
/// native orchestrator and a native session an agent started in the directory; and the
/// space's directory.
fn seeded(test: &str) -> (Flow, PrivateRoot, PathBuf) {
    let flow = flow(test);
    let root = PrivateRoot::new(&format!("{test}-root"));
    let config = fs::read_to_string(&flow.config).unwrap();
    let spaces = format!("\n[spaces]\nroot = \"{}\"\n", root.0.display());
    fs::write(&flow.config, config + &spaces).unwrap();
    let directory = root.0.join("fix-login");
    fs::create_dir(&directory).unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
    let made = fs::symlink_metadata(&directory).unwrap();
    let state = emergency_dir(&flow.project).parent().unwrap().to_path_buf();
    fs::create_dir_all(&state).unwrap();
    fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
    let sessions = [
        session_line(ORCHESTRATOR, "fix-login", &directory, None),
        session_line(AGENTS, "helper", &directory, Some(ORCHESTRATOR)),
    ];
    write_private(&state, "sessions", &(sessions.join("\n") + "\n"));
    let project = fs::canonicalize(&flow.project).unwrap();
    let space = format!(
        r#"{{"id":"{}","name":"fix-login","description":"Fix login","title":"Fix login","issue":"","members":[{{"root":"{}"}}],"directory":"{}","directory_identity":[{},{}],"archived":false,"orchestrator":"{ORCHESTRATOR}","earlier":[],"worktrees":[],"created":1,"changed":1}}"#,
        "9".repeat(32),
        project.display(),
        directory.display(),
        made.dev(),
        made.ino(),
    );
    write_private(&state, "spaces", &(space + "\n"));
    (flow, root, directory)
}

/// The tracker's state of hosted session `name`, as the dev state gives it.
fn tracked<'a>(state: &'a serde_json::Value, name: &str) -> Option<&'a str> {
    (state["sessions"]["sessions"].as_array())?
        .iter()
        .find(|session| session["name"] == name && session["key"].get("hosted").is_some())?["state"]
        .as_str()
}

/// Selects buffer `name`, once oso shows it.
fn select(app: &mut App, name: &str) {
    app.send_keys("C-x b");
    app.send_keys(&keys(name));
    app.send_keys("RET");
    app.wait_for(name, |state| state["buffer"]["name"] == name);
}

/// The orchestrator, resumed from its stopped transcript: run `n`'s arguments, once its
/// composer is selected, and back in its transcript.
fn resumed(flow: &Flow, app: &mut App, n: usize) -> Vec<String> {
    select(app, "*claude: fix-login*");
    app.run_command("session-resume", None).unwrap();
    let native = args(flow, n);
    app.wait_for("the composer", in_composer);
    app.send_keys("C-c C-o");
    app.wait_for("the transcript", |state| {
        state["buffer"]["name"] == "*claude: fix-login*"
    });
    native
}

/// Whether `native` is a native line that resumes or copies the orchestrator's conversation
/// with the person's own settings alone.
fn native_in_space(native: &[String]) -> bool {
    native.first().is_some_and(|arg| arg == "-p")
        && flag(native, "resume") == Some(ORCHESTRATOR)
        && native.iter().any(|arg| arg == USER_ONLY)
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn an_orchestrator_resumes_and_forks_natively_in_its_spaces_directory() {
    let (flow, _root, _) = seeded("native-space-resume");
    let mut app = flow.start();
    let native = resumed(&flow, &mut app, 1);
    assert!(native_in_space(&native), "{native:?}");
    let shown = text(&app.wait_for("the row", |state| text(state).contains("Resumed.")));
    assert!(!shown.contains("did not"), "{shown}");

    // A fork of it runs natively there too, judged by the orchestrator it copies.
    app.run_command("session-fork", None).unwrap();
    let fork = args(&flow, 2);
    assert!(native_in_space(&fork), "{fork:?}");
    assert!(fork.iter().any(|arg| arg == "--fork-session"), "{fork:?}");

    // A session an agent started in the directory does not resume natively there.
    select(&mut app, "*claude: helper*");
    app.run_command("session-resume", None).unwrap();
    app.wait_for("the refusal", |state| {
        state["echo"].as_str().is_some_and(|echo| {
            echo.starts_with(
                "Session helper is in a space's directory, but is not its orchestrator",
            )
        })
    });
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert!(!flow.scratch.path().join("run.3").exists(), "no third run");
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn an_orchestrator_switches_to_the_tui_and_back_in_its_spaces_directory() {
    let (flow, _root, _) = seeded("native-space-switch");
    let mut app = flow.start();
    resumed(&flow, &mut app, 1);

    // To the TUI: its line reads the directory's settings as any TUI orchestrator's does.
    app.run_command("session-switch-mode", None).unwrap();
    let tui = args(&flow, 2);
    assert!(
        !tui.iter().any(|arg| arg == "-p" || arg == USER_ONLY),
        "{tui:?}"
    );
    assert_eq!(flag(&tui, "resume"), Some(ORCHESTRATOR), "{tui:?}");
    app.wait_for("the TUI idle", |state| {
        state["buffer"]["name"] == "*claude: fix-login*"
            && screen(state).contains("Claude Code")
            && tracked(state, "fix-login") == Some("idle")
    });

    // And back: native again there, under the space's gate.
    switch_back(&mut app);
    let back = args(&flow, 3);
    assert!(native_in_space(&back), "{back:?}");
    app.wait_for("the composer", in_composer);
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}
