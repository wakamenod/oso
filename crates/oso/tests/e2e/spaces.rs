//! Issue spaces, step 1a (ADR 0136): the sessions an earlier oso remembered, in two
//! repositories and a worktree of one, become two spaces of one repository each, the worktree
//! held by its repository's, with nothing started and no space directory made; the `spaces`
//! file keeps them, so that the next start reads the same spaces and makes none again; and
//! `space-switch` goes from one to the other.
//!
//! Must not depend on: the app's internals, or what the spaces' own tests cover: the join
//! rule, the file's format and the directory checks.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::worktrees::{Flow, keys};

/// oso's state directory under `home`, as the platform has it.
fn state_dir(home: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/oso")
    } else {
        home.join(".local/state/oso")
    }
}

/// A remembered session's line: its id from `n`, its name, and where it ran.
fn session_line(n: u8, name: &str, place: &Path) -> String {
    let id = format!("{n:08x}-0000-4000-8000-000000000000");
    let place = place.display();
    format!(
        r#"{{"id":"{id}","name":"{name}","directory":"{place}","root":"{place}","origin":"person","pinned":false,"archived":false,"changed":1794512815000}}"#
    )
}

/// The spaces the dev state lists: each one's name, repositories and worktrees.
fn spaces(state: &Value) -> Vec<(String, Vec<String>, Vec<String>)> {
    let texts = |value: &Value| -> Vec<String> {
        value
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    state["sessions"]["spaces"]
        .as_array()
        .map(|spaces| {
            spaces
                .iter()
                .map(|space| {
                    let name = space["name"].as_str().unwrap_or_default().to_owned();
                    (name, texts(&space["members"]), texts(&space["worktrees"]))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The ids of the spaces the dev state lists.
fn space_ids(state: &Value) -> Vec<String> {
    state["sessions"]["spaces"]
        .as_array()
        .map(|spaces| {
            spaces
                .iter()
                .filter_map(|space| space["id"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// Waits for `path` to exist, as the worker writes it.
fn wait_for_file(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "{} was not written",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn remembered_sessions_become_spaces_of_one_repository_that_a_restart_keeps() {
    let flow = Flow::new("spaces-migrate", "");
    // A worktree of the repository, as oso makes them, and another repository.
    let side = flow.repo.join(".claude/worktrees/side");
    fs::create_dir_all(side.parent().unwrap()).unwrap();
    let side_text = side.to_str().unwrap();
    flow.git(
        &flow.repo,
        &["worktree", "add", "-q", "-b", "side", side_text],
    );
    let other = flow.scratch.path().join("other");
    fs::create_dir(&other).unwrap();
    flow.git(&other, &["init", "-q", "-b", "main"]);
    let canonical = |path: &Path| fs::canonicalize(path).unwrap();
    let (repo, side, other) = (canonical(&flow.repo), canonical(&side), canonical(&other));

    // What an earlier oso remembered: no `spaces` file yet.
    let state = state_dir(&flow.home);
    fs::create_dir_all(&state).unwrap();
    fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
    let lines = [
        session_line(1, "repo", &repo),
        session_line(2, "side", &side),
        session_line(3, "other", &other),
    ];
    let record = state.join("sessions");
    fs::write(&record, lines.join("\n") + "\n").unwrap();
    fs::set_permissions(&record, fs::Permissions::from_mode(0o600)).unwrap();

    let (mut app, _) = flow.start();
    let found = app.wait_for("two spaces from the sessions", |state| {
        spaces(state).len() == 2
    });
    let text = |path: &Path| path.display().to_string();
    let mut listed = spaces(&found);
    listed.sort();
    assert_eq!(
        listed,
        [
            ("other".to_owned(), vec![text(&other)], vec![]),
            ("repo".to_owned(), vec![text(&repo)], vec![text(&side)]),
        ]
    );
    let sessions = found["sessions"]["sessions"].as_array().unwrap();
    assert!(
        sessions.iter().all(|s| s["state"] == "stopped"),
        "nothing started: {sessions:?}"
    );
    assert!(
        !flow.home.join("spaces").exists(),
        "no space directory made"
    );
    wait_for_file(&state.join("spaces"));
    let ids = {
        let mut ids = space_ids(&found);
        ids.sort();
        ids
    };

    // `space-switch` to the other repository's space, and back.
    for name in ["other", "repo"] {
        app.send_keys(&format!("C-x a j {} RET", keys(name)));
        app.wait_for("the space in front", |state| {
            state["sessions"]["space"] == name
        });
    }
    let _ = app.exchange("exit", None);
    app.wait_for_exit();

    // The next start reads the same spaces, and makes none again.
    let (mut again, _) = flow.start();
    let back = again.wait_for("the kept spaces", |state| spaces(state).len() == 2);
    let mut back_ids = space_ids(&back);
    back_ids.sort();
    assert_eq!(back_ids, ids);
    flow.exit_with(0);
}
