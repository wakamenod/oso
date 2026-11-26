//! What changed in a pull request since the person's last review of it, in a running instance
//! (ADR 0167, step 2), against the stand-in `gh` of `review_pr`: `M-x review-checkpoint`
//! records the head, a commit pushed after it and `g` read again, the commit list's second row
//! is what changed since, the new commit marked; `RET` there reviews only that, and `M-n` from
//! it steps to the new commit.
//!
//! Must not depend on: the app's internals, a real `gh` or the network: the pull request's
//! commits are this clone's own, so nothing is fetched.

use std::fs;

use super::git_pr::{text, wait_echo, wait_text};
use super::harness::Scratch;
use super::review::git;
use super::review_commits::pull_request_of_two;
use super::review_pr::tip;
use super::typing;

const WHOLE: &str = "*review: pull request #7 proj-git*";

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_checkpoint_shows_what_changed_since_and_m_n_steps_to_the_new_commit() {
    let scratch = Scratch::new("review-since");
    let (mut app, _first, second) = pull_request_of_two(&scratch);
    app.send_keys(&format!("C-x b {} RET", typing("a.txt")));
    app.wait_for("the file", |state| state["buffer"]["name"] == "a.txt");
    app.send_keys("C-u C-c a v");
    app.wait_for("the review menu", |state| state["menu"] == "review");
    app.send_keys("p 7 RET");
    app.wait_for("the review", |state| state["buffer"]["name"] == WHOLE);
    app.wait_for("its commits", |state| {
        state["modeline"]
            .as_str()
            .is_some_and(|modeline| modeline.contains("#7 all 2 commits"))
    });

    // The head recorded as the last review.
    app.send_keys(&format!("M-x {} RET", typing("review-checkpoint")));
    wait_echo(
        &mut app,
        "the checkpoint",
        &format!("Recorded {}", &second[..7]),
    );

    // A commit pushed after it, which gh now names as the head.
    let root = scratch.path().join("proj-git");
    let tree = scratch.path().join("feature-tree");
    fs::write(tree.join("d.txt"), "dee\n").unwrap();
    git(&tree, &["add", "d.txt"]);
    git(&tree, &["commit", "-q", "-m", "Add d"]);
    let third = tip(&root, "feature");
    let bin = scratch.path().join("bin");
    let sides = fs::read_to_string(bin.join("sides.json")).unwrap();
    fs::write(bin.join("sides.json"), sides.replace(&second, &third)).unwrap();
    app.send_keys("g");
    app.wait_for("the new head's commits", |state| {
        state["modeline"]
            .as_str()
            .is_some_and(|modeline| modeline.contains("#7 all 3 commits"))
    });

    // The list's second row: what changed since, one new commit, marked.
    app.send_keys("l");
    let list = wait_text(&mut app, "what changed since", "Since last review");
    let rows: Vec<&str> = list.lines().collect();
    assert!(
        rows[1].ends_with("Since last review \u{b7} 1 new commit"),
        "{list}"
    );
    assert!(rows[4].starts_with("  3/3 new"), "{list}");
    assert!(!rows[2].contains(" new "), "{list}");

    // `RET` on it: the review of only what changed since.
    app.send_keys("M-< C-n RET");
    let since = format!(
        "*review: #7 since {}..{} proj-git*",
        &second[..12],
        &third[..12]
    );
    app.wait_for("what changed since", |state| {
        state["buffer"]["name"] == since
    });
    let shown = wait_text(&mut app, "its line", "+dee");
    assert!(
        !shown.contains("+sea") && !shown.contains("+bee"),
        "{shown}"
    );
    assert!(
        app.state()["modeline"]
            .as_str()
            .is_some_and(|modeline| modeline.contains("#7 since last review")),
        "{}",
        text(&mut app)
    );

    // `M-n` from it: the new commit.
    app.send_keys("M-n");
    wait_echo(&mut app, "the step", "Reviewing #7, commit 3/3 ");
}
