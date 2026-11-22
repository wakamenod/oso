//! Reviewing a pull request a commit at a time in a running instance (ADR 0167), against the
//! stand-in `gh` of `review_pr`: `M-n` steps from the whole pull request to its first commit,
//! whose review shows its message under the title; a comment there shows in the commit list
//! `l` opens above the review; `SPC` and `RET` there show the second commit; `M-p` steps back
//! to the whole; and `C-c C-c` gathers the comments of the whole and of the commit, a group
//! each, the whole's first.
//!
//! Must not depend on: the app's internals, a real `gh` or the network: the pull request's
//! commits are this clone's own, so nothing is fetched.

use std::fs;

use super::git_pr::{status_with_gh, text, wait_echo, wait_text};
use super::git_remote::identity;
use super::git_status::project;
use super::harness::{App, Scratch};
use super::review::git;
use super::review_pr::{stand_in_gh, tip};
use super::typing;

const WHOLE: &str = "*review: pull request #7 proj-git*";

/// The name of the review of the commit `id`, as a step opens it.
fn commit_review(id: &str) -> String {
    format!("*review: commit {} proj-git*", &id[..12])
}

/// An instance on a project whose pull request #7, `feature` in a worktree of its own, adds
/// b.txt and then c.txt, a commit each; with the two commits' ids.
pub(super) fn pull_request_of_two(scratch: &Scratch) -> (App, String, String) {
    let file = project(scratch);
    let root = scratch.path().join("proj-git");
    identity(&root);
    git(
        &root,
        &["remote", "add", "origin", "https://github.com/o/r.git"],
    );
    // `feature`, in a worktree of its own, adds b.txt, then c.txt.
    let tree = scratch.path().join("feature-tree");
    let tree_text = tree.to_str().unwrap();
    git(
        &root,
        &["worktree", "add", "-q", "-b", "feature", tree_text],
    );
    fs::write(tree.join("b.txt"), "bee\n").unwrap();
    git(&tree, &["add", "b.txt"]);
    git(&tree, &["commit", "-q", "-m", "Add b\n\nIt holds a bee."]);
    let first = tip(&root, "feature");
    fs::write(tree.join("c.txt"), "sea\n").unwrap();
    git(&tree, &["add", "c.txt"]);
    git(&tree, &["commit", "-q", "-m", "Add c"]);
    let second = tip(&root, "feature");
    let bin = stand_in_gh(scratch, &tip(&root, "main"), &second);
    (status_with_gh(scratch, &file, &bin), first, second)
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn m_n_steps_to_each_commit_and_c_c_c_c_sends_them_grouped() {
    let scratch = Scratch::new("review-commits");
    let (mut app, first, second) = pull_request_of_two(&scratch);

    // The whole pull request, from the review menu's `p`.
    // From the project's file, as `review_pr` opens the menu: the status's own keys aside.
    app.send_keys(&format!("C-x b {} RET", typing("a.txt")));
    app.wait_for("the file", |state| state["buffer"]["name"] == "a.txt");
    app.send_keys("C-u C-x v r");
    app.wait_for("the review menu", |state| state["menu"] == "review");
    app.send_keys("p 7 RET");
    app.wait_for("the review", |state| state["buffer"]["name"] == WHOLE);
    wait_echo(&mut app, "what gh said", "Reviewing #7");
    wait_text(&mut app, "the pull request's lines", "+sea");
    app.wait_for("its commits", |state| {
        state["modeline"]
            .as_str()
            .is_some_and(|modeline| modeline.contains("#7 all 2 commits"))
    });

    // `M-n`: its first commit, alone, with its message under the title.
    app.send_keys("M-n");
    let one = commit_review(&first);
    app.wait_for("the first commit", |state| state["buffer"]["name"] == one);
    wait_echo(&mut app, "the step", "Reviewing #7, commit 1/2 ");
    let shown = wait_text(&mut app, "its message", "    It holds a bee.");
    assert!(shown.contains("    Add b"), "{shown}");
    let shown = wait_text(&mut app, "its line", "+bee");
    assert!(!shown.contains("+sea"), "{shown}");
    assert!(
        app.state()["modeline"]
            .as_str()
            .is_some_and(|modeline| modeline.contains("#7 1/2")),
        "the modeline says which commit"
    );

    // A comment on its added line.
    app.send_keys("M-< n C-n c");
    app.send_keys(&format!("{} SPC {} C-c C-c", typing("Why"), typing("bee")));
    wait_text(&mut app, "the comment", "Why bee");

    // `l`: the list, above the review, selected, the comment counted.
    app.send_keys("l");
    app.wait_for("the list", |state| {
        state["buffer"]["name"]
            .as_str()
            .is_some_and(|name| name.ends_with("\u{b7} commits*"))
    });
    let list = wait_text(&mut app, "the comment's mark", "\u{25cf}1");
    let rows: Vec<&str> = list.lines().collect();
    assert!(
        rows[0].contains("All changes \u{b7} #7 \u{b7} 2 commits"),
        "{list}"
    );
    assert!(rows[1].starts_with("\u{25b6} 1/2 \u{25cf}1"), "{list}");
    assert!(rows[2].contains("Add c"), "{list}");

    // `SPC` on the second shows it and stays; `RET` selects it.
    app.send_keys("C-n SPC");
    let two = commit_review(&second);
    wait_text(&mut app, "the list following", "\u{25b6} 2/2");
    assert!(
        app.state()["buffer"]["name"]
            .as_str()
            .is_some_and(|name| name.ends_with("\u{b7} commits*")),
        "SPC stays in the list"
    );
    app.send_keys("RET");
    app.wait_for("the second commit", |state| state["buffer"]["name"] == two);
    wait_text(&mut app, "its line", "+sea");

    // `M-p` twice: the first commit, then the whole pull request.
    app.send_keys("M-p");
    app.wait_for("back at the first", |state| state["buffer"]["name"] == one);
    app.send_keys("M-p");
    app.wait_for("the whole", |state| state["buffer"]["name"] == WHOLE);
    wait_echo(&mut app, "the whole said", "Reviewing #7 whole");

    send_grouped(&mut app);
}

/// From the whole pull request: a comment on it, then `C-c C-c` gathers both comments, a
/// group each, the whole's first.
fn send_grouped(app: &mut App) {
    // A comment on the whole, then `C-c C-c`: both, a group each, the whole's first.
    app.send_keys("M-< n C-n c");
    app.send_keys(&format!("{} SPC {}", typing("On"), typing("all")));
    app.send_keys("C-c C-c");
    wait_text(app, "the whole's comment", "On all");
    app.send_keys("C-c C-c");
    app.wait_for("the message", |state| {
        state["buffer"]["name"] == "*review message*"
    });
    let message = text(app);
    let whole_at = message.find("# The whole of #7").expect(&message);
    let commit_at = message.find("# Commit 1/2 ").expect(&message);
    assert!(whole_at < commit_at, "{message}");
    assert!(message.contains("read a commit at a time"), "{message}");
    assert!(message.contains("Comment: On all"), "{message}");
    assert!(message.contains("Comment: Why bee"), "{message}");
    assert!(!message.contains("# Commit 2/2"), "{message}");
    app.send_keys("C-c C-k");
    app.wait_for("the drop", |state| {
        state["buffer"]["name"] != "*review message*"
    });
}
