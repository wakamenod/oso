//! Reviews in a running instance (ADR 0122): a trusted project's uncommitted changes reviewed
//! with `C-x v r w`, its review buffer read whole with each file's header, hunk and lines, `n`
//! moving to the first hunk, `g` reading a file changed meanwhile, and `q` going back; and the
//! person's comments: typed in a draft, kept as a card, edited, dropped to the kill ring,
//! removed and yanked back, and asked about before the review closes.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test. The sections'
//! text, the rows and the keys have their own tests.

use std::ffi::{OsStr, OsString};
use std::path::Path;

use serde_json::{Value, json};

use super::config::set_mode;
use super::harness::{App, Scratch};
use super::{cursor, typing};
use oso_platform::process::{Executable, Sink, Source, Spawn, spawn};

/// Runs git in `root` with `args`, which must succeed, kept from the person's configuration
/// and hooks: git looks for hooks only in an empty directory beside `root`.
fn git(root: &Path, args: &[&str]) {
    let git = Executable::from_path("git", None).expect("git is on PATH");
    let hooks = root.with_file_name("hooks");
    std::fs::create_dir_all(&hooks).expect("cannot make the hooks directory");
    let mut hooks_path = OsString::from("core.hooksPath=");
    hooks_path.push(&hooks);
    let args: Vec<OsString> = ["-c".into(), hooks_path]
        .into_iter()
        .chain(args.iter().map(Into::into))
        .collect();
    let null = OsStr::new(if cfg!(windows) { "NUL" } else { "/dev/null" });
    let set = [
        ("GIT_CONFIG_GLOBAL", null),
        ("GIT_CONFIG_NOSYSTEM", OsStr::new("1")),
    ];
    let status = spawn(&Spawn {
        executable: &git,
        args: &args,
        cwd: Some(root),
        project: None,
        inherit: &["HOME"],
        stdin: Source::Closed,
        stdout: Sink::Null,
        stderr: Sink::Null,
        own_process_group: false,
        set: &set,
        terminal: None,
    })
    .and_then(|mut child| child.wait())
    .expect("cannot run git");
    assert!(status.success(), "git {args:?}");
}

/// The current buffer's first `rows` rows.
fn rows(app: &mut App, rows: usize) -> Vec<String> {
    let text = app.call("get-text", Some(json!({ "start_row": 0, "rows": rows })));
    text["rows"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// A committed project, `proj-review`, with `a.rs` changed and `b.rs` new in its work tree;
/// returns `a.rs`.
fn project(scratch: &Scratch) -> std::path::PathBuf {
    let file = scratch.write("proj-review/a.rs", b"one\ntwo\nthree\n");
    let root = scratch.path().join("proj-review");
    git(&root, &["init", "-q"]);
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
    std::fs::write(&file, b"one\n2\nthree\n").unwrap();
    scratch.write("proj-review/b.rs", b"fn b() {}\n");
    file
}

/// The current buffer's first rows once one of them is `row`, or the last read after 5 s.
fn rows_until(app: &mut App, row: &str) -> Vec<String> {
    let mut seen = Vec::new();
    for _ in 0..200 {
        seen = rows(app, 20);
        if seen.iter().any(|shown| shown == row) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    seen
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_review_of_uncommitted_changes_reads_its_files_moves_refreshes_and_quits() {
    let scratch = Scratch::new("review-flow");
    set_mode(scratch.path(), 0o700);
    let config = scratch.write("config.toml", b"");
    set_mode(&config, 0o644);
    let file = project(&scratch);
    let mut app = App::start_with_config(Some(&file), scratch.path(), &config);
    app.wait_until_ready();
    app.wait_for("the project", |state| {
        state["modeline"]
            .as_str()
            .is_some_and(|modeline| modeline.contains("  proj-review  restricted  "))
    });
    // A restricted project runs no git: the review refuses.
    app.send_keys(&format!("M-x {} RET", typing("review-uncommitted")));
    let state = app.state();
    assert_eq!(
        state["echo"],
        "Reviewing runs git, which this project is not trusted to run: M-x trust-project"
    );
    assert_eq!(state["buffer"]["name"], "a.rs");
    app.send_keys(&format!("M-x {} RET y", typing("trust-project")));
    app.wait_for("the trust", |state| {
        state["echo"]
            .as_str()
            .is_some_and(|echo| echo.starts_with("Trusted "))
    });

    app.send_keys("C-x v r");
    assert_eq!(app.state()["menu"], "review");
    app.send_keys("w");
    let state = app.wait_for("the review", |state| {
        state["buffer"]["name"] == "*review: uncommitted proj-review*"
    });
    assert!(state["menu"].is_null());
    let shown = rows_until(&mut app, "+fn b() {}");
    let expected = [
        "Review \u{b7} uncommitted \u{b7} proj-review",
        "modified  a.rs  +1 \u{2212}1",
        "@@ -2 +2 @@",
        "-two",
        "+2",
        "added  b.rs  +1 \u{2212}0",
        "@@ -0,0 +1 @@",
        "+fn b() {}",
    ];
    assert_eq!(
        &shown[..expected.len().min(shown.len())],
        &expected[..],
        "{shown:?}"
    );

    // `n` goes to the first hunk's header.
    app.send_keys("n");
    assert_eq!(cursor(&app.state()), (2, 0));

    // A file changed meanwhile is read again with `g`.
    std::fs::write(&file, b"one\n2\nthree\nfour\n").unwrap();
    app.send_keys("g");
    let seen = rows_until(&mut app, "+four");
    assert!(seen.iter().any(|row| row == "+four"), "{seen:?}");
    assert!(
        seen.iter().any(|row| row == "modified  a.rs  +2 \u{2212}1"),
        "{seen:?}"
    );

    // `q` goes back to the file.
    app.send_keys("q");
    assert_eq!(app.state()["buffer"]["name"], "a.rs");
}

/// A review of `proj-review`'s uncommitted changes, open and read whole, with the project
/// trusted.
fn open_review(scratch: &Scratch) -> App {
    set_mode(scratch.path(), 0o700);
    let config = scratch.write("config.toml", b"");
    set_mode(&config, 0o644);
    let file = project(scratch);
    let mut app = App::start_with_config(Some(&file), scratch.path(), &config);
    app.wait_until_ready();
    app.wait_for("the project", |state| {
        state["modeline"]
            .as_str()
            .is_some_and(|modeline| modeline.contains("  proj-review  restricted  "))
    });
    app.send_keys(&format!("M-x {} RET y", typing("trust-project")));
    app.wait_for("the trust", |state| {
        state["echo"]
            .as_str()
            .is_some_and(|echo| echo.starts_with("Trusted "))
    });
    app.send_keys("C-x v r w");
    app.wait_for("the review", |state| {
        state["buffer"]["name"] == "*review: uncommitted proj-review*"
    });
    rows_until(&mut app, "+fn b() {}");
    app
}

/// The innermost prompt's label, if one is open.
fn prompt_label(app: &mut App) -> Option<String> {
    let state = app.state();
    let prompts = state["prompts"].as_array()?;
    Some(prompts.last()?["prompt"].as_str()?.to_owned())
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_comment_is_drafted_kept_edited_removed_and_yanked_back() {
    let scratch = Scratch::new("review-comments");
    let mut app = open_review(&scratch);
    // On `+2`, the hunk's added line.
    app.send_keys("n C-n C-n");
    assert_eq!(cursor(&app.state()), (4, 0));
    app.send_keys("c");
    assert_eq!(
        prompt_label(&mut app).as_deref(),
        Some("Comment on a.rs L2 (new)")
    );
    assert_eq!(app.state()["prompts"][0]["kind"], "draft");
    app.send_keys(&format!(
        "{} RET {} C-c C-c",
        typing("first"),
        typing("second")
    ));
    assert_eq!(prompt_label(&mut app), None, "the draft is kept and closed");
    let shown = rows_until(&mut app, "    \u{258d}  second");
    let card = [
        "+2",
        "    \u{258d}You  #0",
        "    \u{258d}  first",
        "    \u{258d}  second",
    ];
    let at = shown.iter().position(|row| row == "+2").expect("the line");
    assert_eq!(&shown[at..at + 4], &card[..], "{shown:?}");
    assert!(
        shown[1].ends_with("  \u{b7} 1 comment"),
        "the header counts it: {shown:?}"
    );

    // `e` on its label: the draft holds its text; `C-g` drops it into the kill ring.
    app.send_keys("C-n");
    assert_eq!(cursor(&app.state()), (5, 0));
    app.send_keys("e");
    assert_eq!(prompt_label(&mut app).as_deref(), Some("Edit #0"));
    assert_eq!(app.state()["prompts"][0]["text"], "first\nsecond");
    app.send_keys("C-g");
    assert_eq!(prompt_label(&mut app), None);
    assert_eq!(app.state()["echo"], "Draft in the kill ring: C-y");

    // Closing asks while the comment was not sent.
    app.send_keys(&format!("M-x {} RET", typing("review-kill")));
    assert_eq!(
        prompt_label(&mut app).as_deref(),
        Some("1 comment of yours will be lost; kill? (y or n)")
    );
    app.send_keys("n");

    // `k` removes it at once, and `C-y` yanks it back as text elsewhere.
    app.send_keys("k");
    assert_eq!(app.state()["echo"], "Removed #0: C-y yanks it");
    let seen = rows_until(&mut app, "+fn b() {}");
    assert!(!seen.iter().any(|row| row.contains("You  #0")), "{seen:?}");
    app.send_keys("q M-> C-y");
    let state = app.state();
    assert_eq!(state["buffer"]["name"], "a.rs");
    let text = rows(&mut app, 20).join("\n");
    assert!(text.ends_with("You  #0\nfirst\nsecond"), "{text:?}");
}
