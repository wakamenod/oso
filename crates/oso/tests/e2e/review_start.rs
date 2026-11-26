//! Starting a review in a space whose orchestrator is recorded (ADR 0163), with `git_agent`'s
//! fake `claude` as the orchestrator: `C-u C-c a v` takes the space's one repository and opens
//! `review-start`'s menu, whose `.` opens the review menu for this checkout, and whose `p`
//! finds the worktree a pull request's head is checked out in, against a stand-in `gh`, and
//! opens its review there.
//!
//! Must not depend on: the app's internals, a real `gh` or the network: the pull request's
//! commits are this clone's own. The default repository, the worktrees listed and the head's
//! worktree have their own tests.

use std::fs;
use std::path::Path;

use super::git_agent::FAKE;
use super::harness::App;
use super::review_pr::{stand_in_gh, tip};
use super::spaces::{PrivateRoot, answer, space};
use super::worktrees::{Flow, terminal};

/// A flow whose space root is private, with git's identity set in its repository.
fn space_flow(test: &str) -> (Flow, PrivateRoot) {
    let spaces = PrivateRoot::new(&format!("{test}-root"));
    let config = format!("\n[spaces]\nroot = \"{}\"\n", spaces.0.display());
    let flow = Flow::with_fake(test, &config, FAKE);
    for (name, value) in [
        ("user.name", "e2e"),
        ("user.email", "e2e@example.invalid"),
        ("commit.gpgsign", "false"),
    ] {
        flow.git(&flow.repo, &["config", name, value]);
    }
    (flow, spaces)
}

/// Makes the space `Fix login` of the repository, and waits for its orchestrator.
fn orchestrated(app: &mut App) {
    app.send_keys("C-c a O");
    answer(app, "Describe the space:", "Fix login");
    app.wait_for("the orchestrator, ready and recorded", |state| {
        space(state, "fix-login").is_some_and(|s| s["orchestrator"].is_string())
            && terminal(state, "*claude: fix-login*").is_some_and(|terminal| {
                (terminal["rows"].as_array())
                    .is_some_and(|rows| rows.iter().any(|row| row == "fake ready"))
            })
    });
}

/// Shows `file`, once its project is known, as the review menu reads it.
fn show(app: &mut App, file: &Path, project: &str) {
    app.open_file(file);
    let name = file.file_name().unwrap().to_str().unwrap().to_owned();
    let words = format!("  {project}  ");
    app.wait_for("the file in its project", |state| {
        state["buffer"]["name"] == name.as_str()
            && state["modeline"]
                .as_str()
                .is_some_and(|line| line.contains(&words))
    });
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn in_a_space_c_x_v_r_takes_its_repository_and_dot_reviews_this_checkout() {
    let (flow, _spaces) = space_flow("review-start");
    fs::write(flow.repo.join("README.md"), "# e2e\nchanged\n").unwrap();
    let (mut app, _) = flow.start();
    orchestrated(&mut app);
    show(&mut app, &flow.repo.join("README.md"), "repo");

    // One repository: no prompt for it, and `review-start`'s menu.
    app.send_keys("C-u C-c a v");
    app.wait_for("review-start's menu", |state| {
        state["menu"] == "review-start"
    });
    app.send_keys(".");
    app.wait_for("the review menu", |state| state["menu"] == "review");
    app.send_keys("w");
    app.wait_for("the review", |state| {
        state["buffer"]["name"] == "*review: uncommitted repo*"
    });
    flow.exit_with(0);
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn in_a_space_p_opens_a_pull_request_on_the_worktree_its_head_is_checked_out_in() {
    let (flow, _spaces) = space_flow("review-start-pr");
    let repo = &flow.repo;
    flow.git(
        repo,
        &["remote", "set-url", "origin", "https://github.com/o/r.git"],
    );
    // `feature`, checked out in a worktree of its own, adds b.txt.
    let tree = flow.scratch.path().join("feature-tree");
    let tree_text = tree.to_str().unwrap();
    flow.git(repo, &["worktree", "add", "-q", "-b", "feature", tree_text]);
    fs::write(tree.join("b.txt"), "from the pull request\n").unwrap();
    flow.git(&tree, &["add", "b.txt"]);
    flow.git(&tree, &["commit", "-q", "-m", "Add b"]);
    let bin = stand_in_gh(&flow.scratch, &tip(repo, "main"), &tip(repo, "feature"));
    let gh = bin.join("gh");
    let (mut app, _) = flow.start_with(&[("OSO_DEV_GH", gh.as_os_str())]);
    orchestrated(&mut app);
    show(&mut app, &repo.join("README.md"), "repo");

    app.send_keys("C-u C-c a v");
    app.wait_for("review-start's menu", |state| {
        state["menu"] == "review-start"
    });
    app.send_keys("p");
    app.wait_for("the number's prompt", |state| {
        super::worktrees::asks(state).is_some_and(|p| p.starts_with("Review pull request #"))
    });
    app.send_keys("7 RET");
    let name = "*review: pull request #7 feature-tree*";
    app.wait_for("the review on the head's worktree", |state| {
        state["buffer"]["name"] == name
    });
    // Its new side is the worktree's own work tree.
    let echo = |state: &serde_json::Value| state["echo"].as_str().unwrap_or_default().to_owned();
    let state = app.wait_for("what gh said", |state| {
        echo(state).starts_with("Reviewing #7")
    });
    assert!(
        echo(&state).contains("checked out here, so the work tree shows"),
        "{}",
        echo(&state)
    );
    flow.exit_with(0);
}
