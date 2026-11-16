//! Stashes and branches from a git status in a running instance (ADR 0127, step 3b): `z - u z`
//! stashes the changes with a message and the untracked file, the Stashes section shows it,
//! `z p` on its row pops it, `b c` makes a branch at `HEAD` and checks it out, `b b` checks out
//! the first again, and `b k` deletes the one merged.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test. The stash list,
//! finding a stash again, and planning a delete have their own tests.

use super::git_remote::identity;
use super::git_status::{has, project, rows_until, trusted_status};
use super::harness::App;
use super::typing;

fn wait_echo(app: &mut App, what: &str, said: &str) {
    app.wait_for(what, |state| state["echo"] == said);
}

/// Waits for the prompt that starts with `label`.
fn wait_prompt(app: &mut App, label: &str) {
    app.wait_for(label, |state| {
        state["prompts"][0]["prompt"]
            .as_str()
            .is_some_and(|prompt| prompt.starts_with(label))
    });
}

fn head_is(rows: &[String], branch: &str) -> bool {
    rows.first()
        .is_some_and(|head| head.starts_with(&format!("Head:     {branch} ")))
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_stash_is_made_and_popped_and_a_branch_made_checked_out_and_deleted() {
    let scratch = super::harness::Scratch::new("git-stash-branch");
    let file = project(&scratch);
    identity(&scratch.path().join("proj-git"));
    let mut app = trusted_status(&scratch, &file);
    rows_until(&mut app, |rows| has(rows, "Untracked files (1)"));
    // `- u` takes the untracked file too; the message is asked first.
    app.send_keys("z - u z");
    wait_prompt(&mut app, "Stash message:");
    app.send_keys(&typing("tidy"));
    app.send_keys("RET");
    wait_echo(&mut app, "the stash", "Stashed");
    let shown = rows_until(&mut app, |rows| has(rows, "Stashes (1)"));
    assert!(has(&shown, "stash@{0} On main: tidy"), "{shown:?}");
    assert!(!has(&shown, "Untracked files (1)"), "{shown:?}");
    // `z p` on the stash's row pops it.
    app.send_keys("M-< n C-n");
    app.send_keys("z p");
    wait_echo(&mut app, "the pop", "Popped the stash On main: tidy");
    let shown = rows_until(&mut app, |rows| {
        !has(rows, "Stashes (1)") && has(rows, "Untracked files (1)")
    });
    assert!(has(&shown, "Unstaged changes (1)"), "{shown:?}");
    // `b c` makes a branch at `HEAD`, offered first, and checks it out.
    app.send_keys("b c");
    wait_prompt(&mut app, "Make the branch at:");
    app.send_keys("RET");
    wait_prompt(&mut app, "Name of the new branch:");
    app.send_keys(&typing("side"));
    app.send_keys("RET");
    wait_echo(&mut app, "the branch", "Made side at HEAD");
    rows_until(&mut app, |rows| head_is(rows, "side"));
    // `b b` checks out the first again.
    app.send_keys("b b");
    wait_prompt(&mut app, "Branch:");
    app.send_keys(&typing("main"));
    app.send_keys("RET");
    wait_echo(&mut app, "the checkout", "Checked out main");
    rows_until(&mut app, |rows| head_is(rows, "main"));
    // `b k` deletes `side`, merged: no question.
    app.send_keys("b k");
    wait_prompt(&mut app, "Branch:");
    app.send_keys(&typing("side"));
    app.send_keys("RET");
    wait_echo(&mut app, "the delete", "Deleted side");
}
