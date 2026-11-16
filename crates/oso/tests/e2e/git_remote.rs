//! Remotes from a git status in a running instance (ADR 0127, step 2), against a bare
//! repository beside the project: `P u` with no upstream asks for a remote and sets it, the
//! commit sections show what is unpushed and recent, `P p` pushes to `@{push}`, `c f` makes a
//! fixup commit for the commit at point, `P - f p` asks before a forced push, and `f u` then
//! `F u` fetch and pull what another clone pushed.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test. Planning a push,
//! its arguments, the lease and the sections have their own tests.

use std::path::{Path, PathBuf};

use super::git_status::{has, project, rows_until, trusted_status};
use super::harness::{App, Scratch};
use super::review::git;
use super::typing;

/// The repository's own identity, so that no configuration of the machine's is needed.
fn identity(root: &Path) {
    git(root, &["config", "user.name", "e2e"]);
    git(root, &["config", "user.email", "e2e@example.invalid"]);
    git(root, &["config", "commit.gpgsign", "false"]);
}

/// A bare repository beside the project, its remote `origin`.
fn remote(scratch: &Scratch, root: &Path) -> PathBuf {
    let bare = scratch.path().join("remote.git");
    git(
        root,
        &["init", "-q", "--bare", "-b", "main", bare.to_str().unwrap()],
    );
    git(root, &["remote", "add", "origin", bare.to_str().unwrap()]);
    bare
}

fn wait_echo(app: &mut App, what: &str, said: &str) {
    app.wait_for(what, |state| state["echo"] == said);
}

/// Stages every tracked change and commits it as `subject`.
fn commit_all(app: &mut App, subject: &str) {
    app.send_keys("S");
    rows_until(app, |rows| has(rows, "Staged changes (1)"));
    app.send_keys("c c");
    app.wait_for("the message", |state| {
        state["buffer"]["name"] == "*commit: proj-git*"
    });
    app.send_keys(&typing(subject));
    app.send_keys("C-c C-c");
    wait_echo(app, "the commit", &format!("Committed: {subject}"));
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_push_sets_the_upstream_and_the_sections_show_what_it_lacks() {
    let scratch = Scratch::new("git-remote-push");
    let file = project(&scratch);
    let root = scratch.path().join("proj-git");
    identity(&root);
    remote(&scratch, &root);
    let mut app = trusted_status(&scratch, &file);
    rows_until(&mut app, |rows| has(rows, "Recent commits (1)"));
    // No upstream: `u` asks for a remote, and the one picked becomes it.
    app.send_keys("P u");
    app.wait_for("the remote's prompt", |state| {
        state["prompts"][0]["prompt"]
            .as_str()
            .is_some_and(|label| label.starts_with("Push to remote:"))
    });
    app.send_keys("RET");
    wait_echo(&mut app, "the push", "Pushed main to origin/main");
    rows_until(&mut app, |rows| {
        rows.iter()
            .any(|row| row.starts_with("Upstream: origin/main"))
    });
    commit_all(&mut app, "Change");
    let shown = rows_until(&mut app, |rows| has(rows, "Unpushed to origin/main (1)"));
    assert!(has(&shown, "Recent commits (2)"), "{shown:?}");
    // `p` follows `@{push}`, the upstream here.
    app.send_keys("P p");
    wait_echo(&mut app, "the push", "Pushed main to origin/main");
    let shown = rows_until(&mut app, |rows| {
        !rows.iter().any(|row| row.starts_with("Unpushed to"))
    });
    assert!(
        !shown.iter().any(|row| row.starts_with("Unpushed to")),
        "{shown:?}"
    );
    // `$` shows the push's run.
    app.send_keys("$");
    app.wait_for("the log", |state| {
        state["buffer"]["name"] == "*git log: proj-git*"
    });
    let shown = super::review::rows(&mut app, 40);
    assert!(
        shown
            .iter()
            .any(|row| row.starts_with("$ git push -v --progress --set-upstream -- origin")),
        "{shown:?}"
    );
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_fixup_is_force_pushed_after_a_question_and_a_pull_takes_another_clone_s_commit() {
    let scratch = Scratch::new("git-remote-fixup");
    let file = project(&scratch);
    let root = scratch.path().join("proj-git");
    identity(&root);
    let bare = remote(&scratch, &root);
    git(&root, &["push", "-q", "-u", "origin", "main"]);
    let mut app = trusted_status(&scratch, &file);
    rows_until(&mut app, |rows| has(rows, "Recent commits (1)"));
    // `c f` on the recent commit makes a fixup commit for it.
    app.send_keys("S");
    rows_until(&mut app, |rows| has(rows, "Staged changes (1)"));
    app.send_keys("M-> C-p");
    app.send_keys("c f");
    wait_echo(
        &mut app,
        "the fixup",
        "Made a fixup! commit: git rebase -i --autosquash in a shell folds it in",
    );
    rows_until(&mut app, |rows| {
        rows.first()
            .is_some_and(|head| head.ends_with(" fixup! base"))
    });
    // `- f` asks first, naming what it would drop there: nothing.
    app.send_keys("P - f p");
    app.wait_for("the question", |state| {
        state["prompts"][0]["prompt"]
            .as_str()
            .is_some_and(|label| label.starts_with("Force-push main to origin/main?"))
    });
    app.send_keys("y");
    wait_echo(&mut app, "the push", "Pushed main to origin/main");
    // Another clone pushes; `f u` fetches it and `F u` takes it, fast-forward.
    let other = scratch.path().join("other");
    git(
        &root,
        &[
            "clone",
            "-q",
            bare.to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    identity(&other);
    std::fs::write(other.join("b.txt"), "b\n").unwrap();
    git(&other, &["add", "b.txt"]);
    git(&other, &["commit", "-q", "-m", "theirs"]);
    git(&other, &["push", "-q", "origin", "main"]);
    app.send_keys("f u");
    wait_echo(&mut app, "the fetch", "Fetched from the upstream's remote");
    rows_until(&mut app, |rows| has(rows, "Unpulled from origin/main (1)"));
    app.send_keys("F u");
    wait_echo(&mut app, "the pull", "Pulled, fast-forward");
    rows_until(&mut app, |rows| {
        rows.first().is_some_and(|head| head.ends_with(" theirs"))
    });
}
