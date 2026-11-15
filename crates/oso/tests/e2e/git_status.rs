//! A git status in a running instance (ADR 0127, step 1a): refused in a restricted project,
//! then `C-x g` in a trusted one lists its untracked and unstaged files; `n` and `TAB` show a
//! file's hunks; `s` stages one hunk alone, `U` unstages it, `s` on a section stages its
//! files; `RET` visits a hunk's line; `q` goes back.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test. The layout, the
//! staging and the keys have their own tests.

use std::path::{Path, PathBuf};

use super::config::set_mode;
use super::harness::{App, Scratch};
use super::review::{git, rows};
use super::{cursor, typing};

const TEXT: &str = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n13\n14\n15\n";

/// A committed project, `proj-git`, with `a.txt` changed near its start and its end and
/// `new.txt` untracked; returns `a.txt`.
fn project(scratch: &Scratch) -> PathBuf {
    let file = scratch.write("proj-git/a.txt", TEXT.as_bytes());
    let root = scratch.path().join("proj-git");
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["add", "-A"]);
    git(
        &root,
        &[
            "-c",
            "user.name=e2e",
            "-c",
            "user.email=e2e@example.invalid",
            "commit",
            "-q",
            "--no-gpg-sign",
            "-m",
            "base",
        ],
    );
    // Lines 2 and 14 only: a replace of "2\n" would change line 12 too.
    let changed = TEXT
        .replacen("\n2\n", "\ntwo\n", 1)
        .replace("\n14\n", "\nfourteen\n");
    std::fs::write(&file, changed).unwrap();
    scratch.write("proj-git/new.txt", b"new\n");
    file
}

/// The status's rows once `done` holds for them, or the last read after 5 s.
fn rows_until(app: &mut App, done: impl Fn(&[String]) -> bool) -> Vec<String> {
    let mut seen = Vec::new();
    for _ in 0..200 {
        seen = rows(app, 40);
        if done(&seen) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    seen
}

fn has(rows: &[String], row: &str) -> bool {
    rows.iter().any(|shown| shown == row)
}

/// The row point is on.
fn at_point(app: &mut App) -> String {
    let (row, _) = cursor(&app.state());
    let shown = rows(app, 40);
    shown
        .get(usize::try_from(row).unwrap())
        .cloned()
        .unwrap_or_default()
}

fn started(scratch: &Scratch, file: &Path) -> App {
    set_mode(scratch.path(), 0o700);
    let config = scratch.write("config.toml", b"");
    set_mode(&config, 0o644);
    let mut app = App::start_with_config(Some(file), scratch.path(), &config);
    app.wait_until_ready();
    app.wait_for("the project", |state| {
        state["modeline"]
            .as_str()
            .is_some_and(|modeline| modeline.contains("  proj-git  restricted  "))
    });
    app
}

/// An instance on `file`, whose status `C-x g` first refuses, as its project is restricted, then
/// shows once the project is trusted.
fn trusted_status(scratch: &Scratch, file: &Path) -> App {
    let mut app = started(scratch, file);
    // A restricted project runs no git: the status refuses.
    app.send_keys("C-x g");
    let state = app.state();
    assert_eq!(
        state["echo"],
        "The status runs git, which this project is not trusted to run: M-x trust-project"
    );
    assert_eq!(state["buffer"]["name"], "a.txt");
    app.send_keys(&format!("M-x {} RET y", typing("trust-project")));
    app.wait_for("the trust", |state| {
        state["echo"]
            .as_str()
            .is_some_and(|echo| echo.starts_with("Trusted "))
    });
    app.send_keys("C-x g");
    app.wait_for("the status", |state| {
        state["buffer"]["name"] == "*git: proj-git*"
    });
    app
}

/// From a hunk's header, `RET` on its line `+fourteen` visits the file at line 14, and `C-x g`
/// comes back.
fn visit_fourteen(app: &mut App) {
    let line = rows(app, 40)
        .iter()
        .position(|row| row == "+fourteen")
        .unwrap();
    let (row, _) = cursor(&app.state());
    for _ in row..u64::try_from(line).unwrap() {
        app.send_keys("C-n");
    }
    assert_eq!(at_point(app), "+fourteen");
    app.send_keys("RET");
    let state = app.wait_for("the file", |state| state["buffer"]["name"] == "a.txt");
    assert_eq!(cursor(&state).0, 13);
    app.send_keys("C-x g");
    app.wait_for("the status", |state| {
        state["buffer"]["name"] == "*git: proj-git*"
    });
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_status_stages_a_hunk_alone_unstages_it_and_stages_a_section() {
    let scratch = Scratch::new("git-status-flow");
    let file = project(&scratch);
    let mut app = trusted_status(&scratch, &file);
    let shown = rows_until(&mut app, |rows| has(rows, "M  a.txt  +2 -2"));
    assert!(shown[0].starts_with("Head:     main "), "{shown:?}");
    assert!(shown[0].ends_with(" base"), "{shown:?}");
    for row in ["Untracked files (1)", "?  new.txt", "Unstaged changes (1)"] {
        assert!(has(&shown, row), "{row}: {shown:?}");
    }
    // Point starts on the first section's heading; `n` walks the headings.
    assert_eq!(at_point(&mut app), "Untracked files (1)");
    app.send_keys("n n n");
    assert_eq!(at_point(&mut app), "M  a.txt  +2 -2");

    // `TAB` shows the file's two hunks; `n` goes to the first.
    app.send_keys("TAB");
    let shown = rows_until(&mut app, |rows| has(rows, "+fourteen"));
    assert!(has(&shown, "@@ -1,5 +1,5 @@"), "{shown:?}");
    assert_eq!(at_point(&mut app), "M  a.txt  +2 -2");
    app.send_keys("n");
    assert_eq!(at_point(&mut app), "@@ -1,5 +1,5 @@");

    // `s` stages that hunk alone.
    app.send_keys("s");
    let shown = rows_until(&mut app, |rows| has(rows, "Staged changes (1)"));
    assert_eq!(app.state()["echo"], "Staged a hunk");
    assert!(has(&shown, "M  a.txt  +1 -1"), "{shown:?}");
    let unstaged = shown
        .iter()
        .position(|row| row == "Unstaged changes (1)")
        .unwrap();
    let staged = shown
        .iter()
        .position(|row| row == "Staged changes (1)")
        .unwrap();
    assert!(unstaged < staged);
    // The unstaged side still holds the other hunk, open, and point is on it.
    assert!(has(&shown, "+fourteen"), "{shown:?}");
    assert!(!has(&shown, "+two"), "{shown:?}");
    assert!(at_point(&mut app).starts_with("@@ -11,"));

    // `RET` on its added line visits the file there.
    visit_fourteen(&mut app);

    // `U` unstages it all again.
    app.send_keys("U");
    let shown = rows_until(&mut app, |rows| !has(rows, "Staged changes (1)"));
    assert_eq!(app.state()["echo"], "Unstaged a file");
    assert!(has(&shown, "M  a.txt  +2 -2"), "{shown:?}");

    // `s` on the untracked section's heading adds its file.
    app.send_keys("M-<");
    app.send_keys("n");
    assert_eq!(at_point(&mut app), "Untracked files (1)");
    app.send_keys("s");
    let shown = rows_until(&mut app, |rows| has(rows, "A  new.txt  +1 -0"));
    assert!(!has(&shown, "Untracked files (1)"), "{shown:?}");

    // `q` goes back to the file.
    app.send_keys("q");
    assert_eq!(app.state()["buffer"]["name"], "a.txt");
}
