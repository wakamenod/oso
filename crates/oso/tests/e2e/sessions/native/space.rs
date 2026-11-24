//! A space's orchestrator as a native session in the space's own directory (ADR 0176): its
//! resume, a fork of it and its switches to the TUI and back run natively there under the
//! space's gate, reading the person's own settings alone (`--setting-sources=user`), and the
//! TUI's line reads the directory's as before; a session an agent started there is refused.
//! A repository trusted while it runs is given by a resume with it, at the person's `y`, and a
//! review's question to it, stopped, resumes it and goes once it runs (part O-2). A new
//! orchestrator, by `space-orchestrator-start` or as a successor, starts natively there too
//! (part O-3).
//! The space and its sessions are a record written before oso starts, in the flow's own state
//! directory, with the space's directory under a private root of the flow's.
//!
//! Must not depend on: the app's internals. Driven by its control socket and the fake's pty
//! alone; no real key event is posted.

use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use oso_platform::process::{Executable, Sink, Source, Spawn, spawn};

use super::super::super::git_agent::echo;
use super::super::super::git_status::{has, rows_until};
use super::super::super::harness::{App, emergency_dir};
use super::super::super::spaces::{PrivateRoot, switch_space, trust_restricted};
use super::super::super::worktrees::{asks, keys};
use super::switch::{args, flag, flow, screen, switch_back};
use super::{Flow, in_composer, joined, remote_control_off, text};

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
    // The member with its identity, as oso records one, by which a trusted one is judged.
    let member = fs::symlink_metadata(&project).unwrap();
    let space = format!(
        r#"{{"id":"{}","name":"fix-login","description":"Fix login","title":"Fix login","issue":"","members":[{{"root":"{}","identity":[{},{}]}}],"directory":"{}","directory_identity":[{},{}],"archived":false,"orchestrator":"{ORCHESTRATOR}","earlier":[],"worktrees":[],"created":1,"changed":1}}"#,
        "9".repeat(32),
        project.display(),
        member.dev(),
        member.ino(),
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

/// `native-remote-control` in the buffer in front refuses, as `why` starts, with no question.
fn remote_refused(app: &mut App, why: &str) {
    app.run_command("native-remote-control", None).unwrap();
    let state = app.wait_for(why, |state| {
        state["echo"]
            .as_str()
            .is_some_and(|echo| echo.starts_with(why))
    });
    assert!(state["card"]["question_takes_answer"].is_null(), "{state}");
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

    // Neither it nor an agent's session is connected to claude.ai (ADR 0181).
    remote_refused(&mut app, "Session fix-login is an orchestrator");

    // A fork of it runs natively there too, judged by the orchestrator it copies.
    app.run_command("session-fork", None).unwrap();
    let fork = args(&flow, 2);
    assert!(native_in_space(&fork), "{fork:?}");
    assert!(fork.iter().any(|arg| arg == "--fork-session"), "{fork:?}");

    // A session an agent started in the directory does not resume natively there.
    select(&mut app, "*claude: helper*");
    remote_refused(&mut app, "Session helper was started by an agent");
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
    app.wait_for("the composer docked", super::composer_docked);
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}

/// Runs `git` with `args` in `dir`, with no global config.
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
        set: &[("GIT_CONFIG_GLOBAL", OsStr::new("/dev/null"))],
        terminal: None,
        pass_fd: None,
        args_logged: true,
    })
    .unwrap();
    assert!(child.wait_with_output().unwrap().status.success());
}

/// Opens the project's `notes.md`, and trusts the project as a person does.
fn trust_project(flow: &Flow, app: &mut App) {
    app.open_file(&flow.project.join("notes.md"));
    trust_restricted(app, "proj");
    app.wait_for("the project trusted", |state| {
        state["modeline"]
            .as_str()
            .is_some_and(|modeline| !modeline.contains("restricted"))
    });
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_repository_trusted_while_a_native_orchestrator_runs_is_given_by_a_resume_with_it() {
    let (flow, _root, _) = seeded("native-space-add-dir");
    let mut app = flow.start();
    let first = resumed(&flow, &mut app, 1);
    assert!(native_in_space(&first), "{first:?}");
    assert_eq!(flag(&first, "add-dir"), None, "{first:?}");

    // The space's repository trusted while it runs: no paste, but a resume with it.
    trust_project(&flow, &mut app);
    app.wait_for("the offer", |state| {
        asks(state).is_some_and(|prompt| prompt.starts_with("Resume orchestrator fix-login with "))
    });
    app.send_keys("y");
    let second = args(&flow, 2);
    assert!(native_in_space(&second), "{second:?}");
    let project = fs::canonicalize(&flow.project).unwrap();
    assert_eq!(flag(&second, "add-dir"), project.to_str(), "{second:?}");
    select(&mut app, "*claude: fix-login*");
    let added = format!("Resumed with {} added.", project.display());
    // It wraps at the narrow window's edge.
    app.wait_for("the row", |state| joined(state).contains(&added));
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_reviews_question_resumes_a_stopped_native_orchestrator_and_goes_once_it_runs() {
    let (flow, _root, _) = seeded("native-space-ask");
    for args in [
        &["add", "-A"][..],
        &[
            "-c",
            "user.name=e2e",
            "-c",
            "user.email=e2e@example.invalid",
            "commit",
            "-q",
            "--no-gpg-sign",
            "-m",
            "base",
        ],
    ] {
        git(&flow.project, args);
    }
    fs::write(flow.project.join("notes.md"), "notes\nchanged\n").unwrap();
    let mut app = flow.start();
    trust_project(&flow, &mut app);
    switch_space(&mut app, "fix-login");
    app.open_file(&flow.project.join("notes.md"));
    // Its project is found off the UI thread; the review menu needs it.
    app.wait_for("notes.md in its project", |state| {
        state["buffer"]["name"] == "notes.md"
            && state["modeline"]
                .as_str()
                .is_some_and(|line| line.contains("  proj  "))
    });
    app.send_keys("C-u C-x v r");
    app.wait_for("review-start's menu", |state| {
        state["menu"] == "review-start"
    });
    app.send_keys(".");
    app.wait_for("the review menu", |state| state["menu"] == "review");
    app.send_keys("w");
    app.wait_for("the review", |state| {
        state["buffer"]["name"] == "*review: uncommitted proj*"
    });
    rows_until(&mut app, |rows| has(rows, "+changed"));

    // `Q` to the stopped native orchestrator offers its resume, and the question waits.
    app.send_keys("M-< n C-n Q");
    app.wait_for("the question's prompt", |state| {
        asks(state).is_some_and(|prompt| prompt.starts_with("Ask about this"))
    });
    app.send_keys(&format!("{} RET", keys("Why")));
    app.wait_for("the resume offer", |state| {
        asks(state).is_some_and(|prompt| {
            prompt.starts_with("Orchestrator fix-login is stopped: resume it and ask it then?")
        })
    });
    app.send_keys("y");
    let native = args(&flow, 1);
    assert!(native_in_space(&native), "{native:?}");

    // Once it runs, the question goes, and the fake answers it.
    app.wait_for("the question sent", |state| {
        echo(state).starts_with("Asked fix-login, sent")
    });
    select(&mut app, "*claude: fix-login*");
    app.wait_for("its answer", |state| text(state).contains("reply 1.1"));
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}

/// The space's record, once its line holds what `holds` looks for.
fn wait_for_space(flow: &Flow, what: &str, holds: impl Fn(&serde_json::Value) -> bool) {
    let place = emergency_dir(&flow.project)
        .parent()
        .unwrap()
        .join("spaces");
    let started = std::time::Instant::now();
    loop {
        let text = fs::read_to_string(&place).unwrap_or_default();
        let line = text
            .lines()
            .next()
            .and_then(|l| serde_json::from_str(l).ok());
        if line.as_ref().is_some_and(&holds) {
            return;
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(20),
            "no {what}: {text}"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// Whether `native` is a native line of a new conversation, with the person's own settings
/// alone and the space's prompt.
fn new_in_space(native: &[String]) -> bool {
    native.first().is_some_and(|arg| arg == "-p")
        && flag(native, "resume").is_none()
        && flag(native, "session-id").is_some()
        && native.iter().any(|arg| arg == USER_ONLY)
        && flag(native, "append-system-prompt").is_some()
        && remote_control_off(native)
}

/// Puts space `fix-login` in front.
fn enter_space(app: &mut App) {
    switch_space(app, "fix-login");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_new_orchestrator_starts_natively_in_its_spaces_directory() {
    let (flow, _root, _) = seeded("native-space-new");
    let mut app = flow.start();
    enter_space(&mut app);
    app.run_command("space-orchestrator-start", None).unwrap();
    let native = args(&flow, 1);
    assert!(new_in_space(&native), "{native:?}");
    // With no message, the person writes the first one in its composer.
    app.wait_for("the composer", in_composer);
    app.send_keys(&keys("hello there"));
    app.send_keys("C-c C-c");
    app.send_keys("C-c C-o");
    app.wait_for("the reply", |state| text(state).contains("reply 1.1"));
    // It is the space's orchestrator now, the one before kept as an earlier one.
    wait_for_space(&flow, "the new orchestrator", |space| {
        space["orchestrator"]
            .as_str()
            .is_some_and(|id| id != ORCHESTRATOR)
            && space["earlier"] == serde_json::json!([ORCHESTRATOR])
    });
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_successor_starts_natively_with_its_brief() {
    let (flow, _root, directory) = seeded("native-space-successor");
    fs::write(directory.join("handoff.md"), "Where things are.\n").unwrap();
    let mut app = flow.start();
    enter_space(&mut app);
    app.run_command("space-orchestrator-replace", None).unwrap();
    let native = args(&flow, 1);
    assert!(new_in_space(&native), "{native:?}");
    // Its brief is its first message, which the fake answers, in its buffer, shown as it
    // starts: the old one's transcript keeps its buffer's name.
    app.wait_for("the reply", |state| {
        (state["buffer"]["name"].as_str())
            .is_some_and(|name| name.starts_with("*claude: fix-login"))
            && text(state).contains("reply 1.1")
    });
    let shown = text(&app.state());
    assert!(shown.contains("You take over"), "{shown}");
    wait_for_space(&flow, "the successor", |space| {
        space["orchestrator"]
            .as_str()
            .is_some_and(|id| id != ORCHESTRATOR)
            && space["earlier"] == serde_json::json!([ORCHESTRATOR])
    });
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn with_mode_tui_a_new_orchestrator_starts_in_a_terminal() {
    let (flow, _root, _) = seeded("native-space-new-tui");
    let config = fs::read_to_string(&flow.config).unwrap();
    let tui = config.replacen("[sessions]\n", "[sessions]\nmode = \"tui\"\n", 1);
    fs::write(&flow.config, tui).unwrap();
    let mut app = flow.start();
    enter_space(&mut app);
    app.run_command("space-orchestrator-start", None).unwrap();
    let line = args(&flow, 1);
    assert!(
        !line.iter().any(|arg| arg == "-p" || arg == USER_ONLY),
        "{line:?}"
    );
    app.wait_for("the TUI", |state| screen(state).contains("Claude Code"));
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}
