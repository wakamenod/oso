//! A stopped rebase from a git status in a running instance (ADR 0127, step 3a): `F - r u` pulls
//! onto a conflict and the head says so, `C-c C-c` is refused while a path is unmerged, `C-c
//! C-k` asks, `n` keeps it and `y` aborts it, and once resolved `s` stages the file and `C-c
//! C-c` goes on.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test. Judging what is
//! stopped, the questions and the steps have their own tests.

use super::git_remote::identity;
use super::git_status::{has, project, rows_until, trusted_status};
use super::harness::{App, Scratch};
use super::review::git;

fn wait_echo_starting(app: &mut App, what: &str, start: &str) {
    app.wait_for(what, |state| {
        state["echo"]
            .as_str()
            .is_some_and(|echo| echo.starts_with(start))
    });
}

fn stopped(rows: &[String]) -> bool {
    rows.iter()
        .any(|row| row.starts_with("Rebasing: main onto "))
}

/// Pulls with `-r` onto the conflict, and waits for the head to show it. The head, not the echo:
/// the conflict rewrites the open `a.txt`, and its "Reloaded" can take the echo area before a
/// poll sees the pull's own words, which the run thread's tests check.
fn pull_onto_the_conflict(app: &mut App) {
    app.send_keys("F - r u");
    let shown = rows_until(app, |rows| stopped(rows) && has(rows, "Unmerged paths (1)"));
    let line = shown.iter().find(|row| row.starts_with("Rebasing: "));
    assert!(
        line.is_some_and(|line| line.contains(", 1 of 1; stopped at ") && line.ends_with(" mine")),
        "{shown:?}"
    );
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_rebasing_pull_stops_aborts_after_a_question_and_goes_on_once_resolved() {
    let scratch = Scratch::new("git-stop-rebase");
    let file = project(&scratch);
    let root = scratch.path().join("proj-git");
    identity(&root);
    let bare = scratch.path().join("remote.git");
    let bare = bare.to_str().unwrap();
    git(&root, &["init", "-q", "--bare", "-b", "main", bare]);
    git(&root, &["remote", "add", "origin", bare]);
    git(&root, &["push", "-q", "-u", "origin", "main"]);
    // Ours changes line 2 of a.txt; another clone's changes it too, and is pushed.
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-q", "-m", "mine"]);
    let other = scratch.path().join("other");
    git(&root, &["clone", "-q", bare, other.to_str().unwrap()]);
    identity(&other);
    let theirs = std::fs::read_to_string(other.join("a.txt"))
        .unwrap()
        .replacen("\n2\n", "\ndeux\n", 1);
    std::fs::write(other.join("a.txt"), theirs).unwrap();
    git(&other, &["commit", "-q", "-am", "theirs"]);
    git(&other, &["push", "-q", "origin", "main"]);
    let mut app = trusted_status(&scratch, &file);
    rows_until(&mut app, |rows| has(rows, "Recent commits (2)"));
    pull_onto_the_conflict(&mut app);
    // Continuing waits for the path to be resolved and staged.
    app.send_keys("C-c C-c");
    wait_echo_starting(&mut app, "the refusal", "1 unmerged path is left");
    // `C-c C-k` asks; `n` keeps the rebase, `y` aborts it.
    app.send_keys("C-c C-k");
    let asked = |state: &serde_json::Value| {
        state["prompts"][0]["prompt"]
            .as_str()
            .is_some_and(|label| label.starts_with("Abort the rebase of main?"))
    };
    app.wait_for("the question", asked);
    app.send_keys("n");
    app.wait_for("no question", |state| !asked(state));
    assert!(stopped(&rows_until(&mut app, stopped)));
    app.send_keys("C-c C-k");
    app.wait_for("the question again", asked);
    app.send_keys("y");
    // The head shows each run's end; their words can lose the echo area to a.txt's reload.
    let shown = rows_until(&mut app, |rows| {
        !stopped(rows) && rows.first().is_some_and(|head| head.ends_with(" mine"))
    });
    assert!(!stopped(&shown), "{shown:?}");
    // Again, resolved this time: `s` on Unmerged paths stages it, and `C-c C-c` goes on.
    pull_onto_the_conflict(&mut app);
    let resolved = std::fs::read_to_string(&file)
        .unwrap()
        .lines()
        .filter(|line| !line.starts_with("<<<<<<<") && !line.starts_with(">>>>>>>"))
        .filter(|line| !line.starts_with("=======") && *line != "deux")
        .map(|line| format!("{line}\n"))
        .collect::<String>();
    std::fs::write(&file, resolved).unwrap();
    app.send_keys("M-< n s");
    rows_until(&mut app, |rows| {
        !has(rows, "Unmerged paths (1)") && has(rows, "Staged changes (1)")
    });
    app.send_keys("C-c C-c");
    let shown = rows_until(&mut app, |rows| {
        !stopped(rows) && rows.first().is_some_and(|head| head.ends_with(" mine"))
    });
    assert!(has(&shown, "Recent commits (3)"), "{shown:?}");
}
