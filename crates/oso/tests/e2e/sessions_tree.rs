//! The tree of `*Sessions*` (ADR 0131, step 2), with worktree sessions' trusted repository and
//! fake `claude`: the project, its main checkout and the session in it; a worktree made with
//! git outside oso shows once the sessions worker sees it; an external session in it is listed
//! under it, and one elsewhere in `Other projects`; `s` shows the list by state and back.
//!
//! Must not depend on: the app's internals, or what the tree's own tests cover: its order,
//! its folds and its faces.

use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use oso_platform::process::{Executable, Sink, Source, Spawn, spawn};
use serde_json::Value;

use super::worktrees::Flow;

/// The list's text, as the dev state gives it.
fn list(state: &Value) -> String {
    state["sessions"]["list"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

/// Whether the list has a row that starts, past its indent, with `start`.
fn has_row(state: &Value, start: &str) -> bool {
    list(state)
        .lines()
        .any(|row| row.trim_start().starts_with(start))
}

/// A process that runs for a minute, for a registry entry to name.
fn sleeper() -> std::process::Child {
    spawn(&Spawn {
        executable: &Executable::absolute("/bin/sleep").unwrap(),
        args: &["60".into()],
        cwd: None,
        project: None,
        inherit: &[],
        stdin: Source::Closed,
        stdout: Sink::Null,
        stderr: Sink::Null,
        own_process_group: false,
        set: &[],
        terminal: None,
    })
    .unwrap()
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn the_tree_shows_the_project_its_worktrees_and_their_sessions() {
    let flow = Flow::new("sessions-tree", "");
    let (mut app, _) = flow.start();
    app.send_keys("C-x a c");
    app.wait_for("the project and its main checkout", |state| {
        has_row(state, "▾ repo") && list(state).lines().any(|row| row.contains("main  primary"))
    });

    // A worktree made with git, outside oso: listed again once the worker sees it.
    let side = flow.repo.join(".claude").join("worktrees").join("side");
    fs::create_dir_all(side.parent().unwrap()).unwrap();
    let side_text = side.to_str().unwrap();
    flow.git(
        &flow.repo,
        &["worktree", "add", "-q", "-b", "side-branch", side_text],
    );
    app.wait_for("the new worktree", |state| {
        has_row(state, "· side  side-branch")
    });

    // An external session in it, and one elsewhere.
    let mut sleeper = sleeper();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let registry = flow.home.join(".claude").join("sessions");
    fs::create_dir_all(&registry).unwrap();
    let elsewhere = flow.scratch.path().join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    let entry = |dir: &std::path::Path, name: &str| {
        format!(
            r#"{{"pid":{},"cwd":"{}","startedAt":{now},"name":"{name}","kind":"interactive","status":"idle"}}"#,
            sleeper.id(),
            dir.display()
        )
    };
    fs::write(
        registry.join(format!("{}.json", sleeper.id())),
        entry(&side, "outside"),
    )
    .unwrap();
    let state = app.wait_for("the external session under its worktree", |state| {
        has_row(state, "○ side") && has_row(state, "○ outside")
    });
    let text = list(&state);
    let rows: Vec<&str> = text.lines().collect();
    let at = rows
        .iter()
        .position(|row| row.trim_start().starts_with("○ side"))
        .unwrap();
    assert!(rows[at + 1].starts_with("    ○ outside"), "{text}");
    fs::write(
        registry.join(format!("{}.json", sleeper.id())),
        entry(&elsewhere, "far"),
    )
    .unwrap();
    app.wait_for("the session elsewhere in Other projects", |state| {
        has_row(state, "▸ Other projects") && !has_row(state, "○ far")
    });

    // `s` in the list: by state, and back to the tree.
    app.send_keys("C-x a s");
    app.wait_for("the list selected", |state| {
        state["buffer"]["name"] == "*Sessions*"
    });
    app.send_keys("s");
    app.wait_for("by state", |state| {
        has_row(state, "▾ Idle") || has_row(state, "▾ Working")
    });
    app.send_keys("s");
    app.wait_for("the tree again", |state| has_row(state, "▾ repo"));

    let _ = sleeper.kill();
    let _ = sleeper.wait();
    flow.exit_with(0);
}
