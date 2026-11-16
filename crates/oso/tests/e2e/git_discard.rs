//! Staging lines and discarding in a running instance (ADR 0127, step 1c): a region on one
//! added line stages it alone; `k` on an untracked file asks, deletes it and keeps a copy;
//! `M-x git-discarded` lists the copy, and `RET` writes it back after its own question.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test. The cutter, the
//! plan, the copies and the record have their own tests.

use super::git_status::{has, project, rows_until, trusted_status};
use super::harness::{App, Scratch};
use super::review::rows;
use super::typing;

fn wait_echo(app: &mut App, what: &str, said: &str) {
    app.wait_for(what, |state| state["echo"] == said);
}

/// Moves point down to the row `row` of the buffer shown.
fn down_to(app: &mut App, row: &str) {
    let at = rows(app, 40).iter().position(|shown| shown == row).unwrap();
    let (now, _) = super::cursor(&app.state());
    for _ in now..u64::try_from(at).unwrap() {
        app.send_keys("C-n");
    }
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_line_stages_alone_and_a_discard_keeps_a_copy_that_comes_back() {
    let scratch = Scratch::new("git-discard-flow");
    let file = project(&scratch);
    let root = scratch.path().join("proj-git");
    let mut app = trusted_status(&scratch, &file);
    rows_until(&mut app, |rows| has(rows, "M  a.txt  +2 -2"));
    // The file's hunks, then a region on `+two` alone.
    app.send_keys("M-<");
    down_to(&mut app, "M  a.txt  +2 -2");
    app.send_keys("TAB");
    rows_until(&mut app, |rows| has(rows, "+two"));
    down_to(&mut app, "+two");
    app.send_keys("C-SPC C-e s");
    let shown = rows_until(&mut app, |rows| has(rows, "Staged changes (1)"));
    assert_eq!(app.state()["echo"], "Staged a hunk's lines");
    assert!(has(&shown, "M  a.txt  +1 -0"), "{shown:?}");
    // `k` on the untracked file: a question, then it goes.
    app.send_keys("M-<");
    down_to(&mut app, "?  new.txt");
    app.send_keys("k");
    let state = app.state();
    let label = state["prompts"][0]["prompt"].as_str().unwrap_or_default();
    assert!(label.starts_with("Delete untracked new.txt?"), "{state}");
    app.send_keys("y");
    wait_echo(
        &mut app,
        "the discard",
        "Discarded a file: M-x git-discarded brings it back",
    );
    assert!(!root.join("new.txt").exists());
    // The copy comes back.
    app.send_keys(&format!("M-x {} RET", typing("git-discarded")));
    app.wait_for("the list", |state| {
        state["buffer"]["name"] == "*git discarded: proj-git*"
    });
    let shown = rows(&mut app, 10);
    assert!(
        shown.get(2).is_some_and(|row| row.ends_with("  new.txt")),
        "{shown:?}"
    );
    app.send_keys("C-n C-n RET");
    let state = app.state();
    let label = state["prompts"][0]["prompt"].as_str().unwrap_or_default();
    assert!(
        label.starts_with("Write new.txt back as it was "),
        "{label}"
    );
    app.send_keys("y");
    wait_echo(&mut app, "the write", "Wrote new.txt back");
    assert_eq!(std::fs::read(root.join("new.txt")).unwrap(), b"new\n");
}
