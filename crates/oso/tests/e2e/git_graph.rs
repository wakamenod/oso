//! The graph of a repository's commits in a running instance (ADR 0127, step 4): `l l` from a
//! git status shows the branch's commits with their lanes and labels, a merge forking and
//! joining, `w` copies a commit's id, `a` shows another branch too, and `RET` reviews a commit.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test. The lanes, the
//! records and the text have their own tests.

use super::git_remote::identity;
use super::git_status::{project, rows_until, trusted_status};
use super::harness::{App, Scratch};
use super::review::{git, rows};

fn wait_echo_starting(app: &mut App, what: &str, start: &str) {
    app.wait_for(what, |state| {
        state["echo"]
            .as_str()
            .is_some_and(|echo| echo.starts_with(start))
    });
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_graph_shows_a_merge_s_lanes_copies_an_id_shows_every_ref_and_reviews_a_commit() {
    let scratch = Scratch::new("git-graph");
    let file = project(&scratch);
    let root = scratch.path().join("proj-git");
    identity(&root);
    // A side branch merged into main, and another branch left apart.
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-q", "-m", "main's"]);
    git(&root, &["checkout", "-q", "-b", "side", "HEAD~"]);
    std::fs::write(root.join("side.txt"), "side\n").unwrap();
    git(&root, &["add", "side.txt"]);
    git(&root, &["commit", "-q", "-m", "side's"]);
    git(&root, &["checkout", "-q", "main"]);
    git(
        &root,
        &["merge", "-q", "--no-ff", "-m", "Merge side", "side"],
    );
    git(&root, &["branch", "apart", "HEAD~"]);
    git(&root, &["checkout", "-q", "apart"]);
    std::fs::write(root.join("apart.txt"), "apart\n").unwrap();
    git(&root, &["add", "apart.txt"]);
    git(&root, &["commit", "-q", "-m", "apart's"]);
    git(&root, &["checkout", "-q", "main"]);
    let mut app = trusted_status(&scratch, &file);
    app.send_keys("l l");
    app.wait_for("the graph", |state| {
        state["buffer"]["name"] == "*graph: proj-git*"
    });
    let shown = rows_until(&mut app, |rows| {
        rows.iter().any(|row| row.contains(" base "))
    });
    assert!(shown[0].starts_with("Commits of HEAD"), "{shown:?}");
    let merge = shown
        .iter()
        .find(|row| row.contains(" Merge side "))
        .expect("the merge");
    assert!(merge.starts_with("●─╮"), "{merge}");
    assert!(merge.contains(" HEAD main Merge side "), "{merge}");
    assert!(
        shown
            .iter()
            .any(|row| row.starts_with("│ ●") && row.contains(" side side's ")),
        "{shown:?}"
    );
    assert!(
        !shown.iter().any(|row| row.contains("apart's")),
        "{shown:?}"
    );
    // `w` on the merge's row copies its id.
    app.send_keys("M-< C-n");
    app.send_keys("w");
    wait_echo_starting(&mut app, "the copy", "Copied ");
    // `a` shows every ref: the branch left apart too.
    app.send_keys("a");
    let shown = rows_until(&mut app, |rows| {
        rows.iter().any(|row| row.contains("apart's"))
    });
    assert!(shown[0].starts_with("Commits of every branch"), "{shown:?}");
    // `RET` reviews the commit at point.
    app.send_keys("M-< C-n RET");
    app.wait_for("the review", |state| {
        state["buffer"]["name"]
            .as_str()
            .is_some_and(|name| name.starts_with("*review"))
    });
    assert!(!rows(&mut app, 5).is_empty());
}
