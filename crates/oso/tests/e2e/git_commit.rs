//! Committing from a git status in a running instance (ADR 0127, step 1b): `c c` opens
//! `*commit: NAME*`, an empty message is refused, `C-c C-c` commits with the repository's hooks
//! and the head shows the new subject, `$` shows the run in `*git log: NAME*`; a failing
//! pre-commit hook keeps the message and is named, `C-c C-k` keeps it for `M-p`, and the menu's
//! `- n` commits past the hook.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test. The arguments,
//! the message's text and the runs have their own tests.

use std::path::Path;

use super::git_status::{has, project, rows_until, trusted_status};
use super::harness::{App, Scratch};
use super::review::git;
use super::typing;

/// The repository's own identity, so that no configuration of the machine's is needed.
fn identity(root: &Path) {
    git(root, &["config", "user.name", "e2e"]);
    git(root, &["config", "user.email", "e2e@example.invalid"]);
    git(root, &["config", "commit.gpgsign", "false"]);
    // Its own hooks, whatever hooks directory the machine's configuration names.
    git(root, &["config", "core.hooksPath", ".git/hooks"]);
}

fn wait_echo(app: &mut App, what: &str, said: &str) {
    app.wait_for(what, |state| state["echo"] == said);
}

fn wait_buffer(app: &mut App, name: &str) {
    app.wait_for(name, |state| state["buffer"]["name"] == name);
}

/// Stages every tracked change, `S`, and waits for it.
fn stage_all(app: &mut App) {
    app.send_keys("S");
    rows_until(app, |rows| has(rows, "Staged changes (1)"));
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_commit_goes_in_and_its_run_shows_in_the_log() {
    let scratch = Scratch::new("git-commit-flow");
    let file = project(&scratch);
    identity(&scratch.path().join("proj-git"));
    let mut app = trusted_status(&scratch, &file);
    rows_until(&mut app, |rows| has(rows, "M  a.txt  +2 -2"));
    stage_all(&mut app);
    app.send_keys("c c");
    wait_buffer(&mut app, "*commit: proj-git*");
    let shown = super::review::rows(&mut app, 20);
    assert!(has(&shown, "#   M  a.txt"), "{shown:?}");
    // Nothing written yet: refused before git runs.
    app.send_keys("C-c C-c");
    wait_echo(
        &mut app,
        "the refusal",
        "The message is empty: write one, or C-c C-k cancels",
    );
    app.send_keys(&format!("{} SPC {}", typing("Change"), typing("a")));
    app.send_keys("C-c C-c");
    wait_echo(&mut app, "the commit", "Committed: Change a");
    // The message's buffer goes, and the status shows the new head.
    wait_buffer(&mut app, "*git: proj-git*");
    let shown = rows_until(&mut app, |rows| {
        rows.first().is_some_and(|head| head.ends_with(" Change a"))
    });
    assert!(!has(&shown, "Staged changes (1)"), "{shown:?}");
    // `$` shows the run, its command line and how it ended.
    app.send_keys("$");
    wait_buffer(&mut app, "*git log: proj-git*");
    let shown = super::review::rows(&mut app, 20);
    assert_eq!(
        shown.first().map(String::as_str),
        Some("$ git -c core.commentChar=# commit --cleanup=strip --file=-"),
        "{shown:?}"
    );
    assert!(
        shown.iter().any(|row| row.starts_with("[done after ")),
        "{shown:?}"
    );
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_failing_hook_keeps_the_message_and_no_verify_commits_past_it() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new("git-commit-hook");
    let file = project(&scratch);
    let root = scratch.path().join("proj-git");
    identity(&root);
    let hook = root.join(".git/hooks/pre-commit");
    std::fs::write(&hook, "#!/bin/sh\necho checks failed\nexit 1\n").unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut app = trusted_status(&scratch, &file);
    rows_until(&mut app, |rows| has(rows, "M  a.txt  +2 -2"));
    stage_all(&mut app);
    app.send_keys("c c");
    wait_buffer(&mut app, "*commit: proj-git*");
    app.send_keys(&typing("Fix"));
    app.send_keys("C-c C-c");
    wait_echo(
        &mut app,
        "the failure",
        "commit failed (hook pre-commit): $ shows the output",
    );
    assert_eq!(app.state()["buffer"]["name"], "*commit: proj-git*");
    // Cancelled, the message is kept for the next one.
    app.send_keys("C-c C-k");
    wait_buffer(&mut app, "*git: proj-git*");
    // The log has what the hook said.
    app.send_keys("$");
    wait_buffer(&mut app, "*git log: proj-git*");
    let shown = super::review::rows(&mut app, 20);
    assert!(has(&shown, "checks failed"), "{shown:?}");
    assert!(
        shown.iter().any(|row| row.starts_with("[failed after ")),
        "{shown:?}"
    );
    app.send_keys("q");
    wait_buffer(&mut app, "*git: proj-git*");
    // `- n` in the menu commits past the hook, with the message `M-p` brings back.
    app.send_keys("c - n c");
    wait_buffer(&mut app, "*commit: proj-git*");
    app.send_keys("M-p");
    let shown = super::review::rows(&mut app, 3);
    assert_eq!(shown.first().map(String::as_str), Some("Fix"), "{shown:?}");
    app.send_keys("C-c C-c");
    wait_echo(&mut app, "the commit", "Committed: Fix");
    rows_until(&mut app, |rows| {
        rows.first().is_some_and(|head| head.ends_with(" Fix"))
    });
}
