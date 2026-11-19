//! Reviewing a pull request in a running instance (ADR 0122, step 12b), against a stand-in
//! `gh` a dev build takes from `OSO_DEV_GH`: `r` on a pull request in `*pull requests*` opens
//! its review, of where its head left the base against its head, which the review menu's `p`
//! shows again, and `gh` is asked about the remote's repository, outside the project.
//!
//! Must not depend on: the app's internals, a real `gh` or the network: the pull request's
//! commits are this clone's own, so nothing is fetched. The fetch, the sides and the worktree
//! found have their own tests.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use super::git_pr::{status_with_gh, wait_echo, wait_text};
use super::git_remote::identity;
use super::git_status::project;
use super::harness::Scratch;
use super::review::git;
use super::typing;

/// A branch's commit, as its loose ref names it.
fn tip(root: &Path, branch: &str) -> String {
    let path = root.join(".git/refs/heads").join(branch);
    fs::read_to_string(path).unwrap().trim().to_owned()
}

/// A `gh` in `bin` that logs where it ran and what it was asked, and answers `pr view` with
/// the pull request #7 from `base` to `head` and `pr list` with it.
fn stand_in_gh(scratch: &Scratch, base: &str, head: &str) -> PathBuf {
    let bin = scratch.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    let sides = format!(
        r#"{{"number":7,"title":"Add b","baseRefName":"main","baseRefOid":"{base}",
        "headRefName":"feature","headRefOid":"{head}","isCrossRepository":false}}"#
    );
    let list = r#"[{"number":7,"title":"Add b","headRefName":"feature","author":{"login":"ann"},
"isDraft":false,"reviewDecision":"","statusCheckRollup":[],"url":"https://github.com/o/r/pull/7"}]"#;
    fs::write(bin.join("sides.json"), sides).unwrap();
    fs::write(bin.join("list.json"), list).unwrap();
    let b = bin.display();
    let script = format!(
        "#!/bin/sh\nprintf '%s | %s\\n' \"$(pwd)\" \"$*\" >> '{b}/asked'\n\
         case \"$1 $2\" in\n\
         'pr view') cat '{b}/sides.json' ;;\n\
         'pr list') cat '{b}/list.json' ;;\n\
         *) exit 1 ;;\nesac\n"
    );
    let gh = bin.join("gh");
    fs::write(&gh, script).unwrap();
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
    bin
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn r_on_a_listed_pull_request_reviews_its_changes_and_the_menu_s_p_shows_it_again() {
    let scratch = Scratch::new("review-pr");
    let file = project(&scratch);
    let root = scratch.path().join("proj-git");
    identity(&root);
    git(
        &root,
        &["remote", "add", "origin", "https://github.com/o/r.git"],
    );
    // `feature`, checked out in a worktree of its own, adds b.txt; `main` moves on after.
    let tree = scratch.path().join("feature-tree");
    let tree_text = tree.to_str().unwrap();
    git(
        &root,
        &["worktree", "add", "-q", "-b", "feature", tree_text],
    );
    fs::write(tree.join("b.txt"), "from the pull request\n").unwrap();
    git(&tree, &["add", "b.txt"]);
    git(&tree, &["commit", "-q", "-m", "Add b"]);
    let head = tip(&root, "feature");
    let bin = stand_in_gh(&scratch, &tip(&root, "main"), &head);
    let mut app = status_with_gh(&scratch, &file, &bin);

    // `N l`, then `r` on the listed one.
    app.send_keys("N l");
    app.wait_for("the list", |state| {
        state["buffer"]["name"] == "*pull requests: proj-git*"
    });
    wait_text(&mut app, "the listed one", "#7");
    app.send_keys("M-< C-n r");
    let name = "*review: pull request #7 proj-git*";
    app.wait_for("the review", |state| state["buffer"]["name"] == name);
    wait_echo(
        &mut app,
        "what gh said",
        "Reviewing #7, its head commit shows \u{b7} feature \u{b7} Add b",
    );
    let shown = wait_text(&mut app, "the pull request's line", "from the pull request");
    assert!(shown.contains("added  b.txt"), "{shown}");
    // The work tree's own changes on `main` are not the pull request's.
    assert!(
        !shown.contains("a.txt") && !shown.contains("new.txt"),
        "{shown}"
    );

    // The review menu's `p` reads the number and shows the same review again: with a review
    // open, `C-u C-x v r` opens the menu (ADR 0160).
    app.send_keys(&format!("C-x b {} RET", typing("a.txt")));
    app.wait_for("the file", |state| state["buffer"]["name"] == "a.txt");
    app.send_keys("C-u C-x v r p 7 RET");
    app.wait_for("the same review", |state| state["buffer"]["name"] == name);
    let names = app.state()["buffers"].to_string();
    assert!(!names.contains("pull request #7 proj-git*<2>"), "{names}");

    // Reading it again asked gh again, about the remote's repository, outside the project.
    let views = |asked: &str| {
        (asked.lines())
            .filter(|line| line.contains("pr view 7"))
            .count()
    };
    let mut asked = String::new();
    for _ in 0..200 {
        asked = fs::read_to_string(bin.join("asked")).unwrap_or_default();
        if views(&asked) >= 2 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(views(&asked) >= 2, "{asked}");
    for line in asked.lines() {
        let (cwd, args) = line.split_once(" | ").unwrap();
        assert!(!Path::new(cwd).starts_with(&root), "{line}");
        assert!(args.contains("--repo github.com/o/r"), "{line}");
    }
}
