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
pub(super) fn git(root: &Path, args: &[&str]) {
    assert!(git_ran(root, args), "git {args:?}");
}

/// Runs git as [`git`] does, saying whether it succeeded.
fn git_ran(root: &Path, args: &[&str]) -> bool {
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
        own_session: false,
        set: &set,
        terminal: None,
        pass_fd: None,
        args_logged: true,
    })
    .and_then(|mut child| child.wait())
    .expect("cannot run git");
    status.success()
}

/// The current buffer's first `rows` rows.
pub(super) fn rows(app: &mut App, rows: usize) -> Vec<String> {
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
    // No language server of the machine's starts in the trusted project (ADR 0138).
    let config = scratch.write("config.toml", b"[lsp]\nstart = \"manual\"\n");
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
    // No language server of the machine's starts in the trusted project (ADR 0138).
    let config = scratch.write("config.toml", b"[lsp]\nstart = \"manual\"\n");
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

    // Closing keeps the comment not sent, as the review is saved (step 6, Q-G): no question,
    // and opening the same scope again brings it back on its line.
    app.send_keys(&format!("M-x {} RET", typing("review-kill")));
    assert_eq!(prompt_label(&mut app), None);
    assert_eq!(
        app.state()["echo"],
        "1 unsent comment kept; review-resume brings them back"
    );
    app.send_keys("C-x v r w");
    app.wait_for("the review again", |state| {
        state["buffer"]["name"] == "*review: uncommitted proj-review*"
    });
    let shown = rows_until(&mut app, "    \u{258d}  second");
    let at = shown.iter().position(|row| row == "+2").expect("the line");
    assert_eq!(&shown[at..at + 4], &card[..], "back on its line: {shown:?}");
    app.send_keys("M-< n C-n C-n C-n");
    assert_eq!(cursor(&app.state()), (5, 0));

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

/// How many windows the frame shows.
fn window_count(app: &mut App) -> usize {
    app.state()["windows"].as_array().map_or(0, Vec::len)
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn bar_shows_the_two_panes_in_step_and_the_one_stream_again() {
    let scratch = Scratch::new("review-panes");
    let mut app = open_review(&scratch);
    // On `+2`, the hunk's added line.
    app.send_keys("n C-n C-n");
    assert_eq!(cursor(&app.state()), (4, 0));

    // `|`: the new side's pane holds the cursor, beside `-two` in the old one.
    app.send_keys("|");
    let state = app.wait_for("the panes", |state| {
        state["buffer"]["name"] == "*review: uncommitted proj-review \u{b7} new*"
    });
    assert_eq!(cursor(&state), (3, 0));
    assert_eq!(window_count(&mut app), 2);
    let new = rows_until(&mut app, "+fn b() {}");
    assert_eq!(new[3], "+2", "{new:?}");
    app.send_keys("C-x o");
    assert_eq!(
        app.state()["buffer"]["name"],
        "*review: uncommitted proj-review \u{b7} old*"
    );
    let old = rows(&mut app, new.len());
    assert_eq!(
        old.len(),
        new.len(),
        "the panes are in step: {old:?} {new:?}"
    );
    assert_eq!(old[3], "-two", "{old:?}");
    // b.rs is new: its added line stands beside a filler row.
    let at = new.iter().position(|row| row == "+fn b() {}").unwrap();
    assert_eq!(old[at], "", "{old:?}");

    // A comment from a pane goes on its side's line.
    app.send_keys("C-x o c");
    assert_eq!(
        prompt_label(&mut app).as_deref(),
        Some("Comment on a.rs L2 (new)")
    );
    app.send_keys("C-g");

    // `|` again: one stream, on the same line.
    app.send_keys("|");
    let state = app.wait_for("the one stream", |state| {
        state["buffer"]["name"] == "*review: uncommitted proj-review*"
    });
    assert_eq!(cursor(&state), (4, 0));
    assert_eq!(window_count(&mut app), 1);

    // Two panes again, then `C-u |`: as the width has it, and this window is under 150 columns.
    app.send_keys("|");
    app.wait_for("the panes again", |state| {
        state["buffer"]["name"] == "*review: uncommitted proj-review \u{b7} new*"
    });
    app.send_keys("C-u |");
    let state = app.wait_for("the one stream by the width", |state| {
        state["buffer"]["name"] == "*review: uncommitted proj-review*"
    });
    assert_eq!(state["echo"], "Two panes or one by the width");
    assert_eq!(window_count(&mut app), 1);
}

/// The echo area's text once it is `said`, or what it said last after 5 s.
fn echo_until(app: &mut App, said: &str) -> String {
    let mut echo = String::new();
    for _ in 0..200 {
        echo = app.state()["echo"].as_str().unwrap_or_default().to_owned();
        if echo == said {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    echo
}

/// The file's text once it is `text`, or what it held last after 5 s.
fn file_until(path: &Path, text: &str) -> String {
    let mut held = String::new();
    for _ in 0..200 {
        held = std::fs::read_to_string(path).unwrap_or_default();
        if held == text {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    held
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_hunk_is_viewed_accepted_rejected_and_each_undone() {
    let scratch = Scratch::new("review-decisions");
    let mut app = open_review(&scratch);
    let root = scratch.path().join("proj-review");
    let file = root.join("a.rs");
    // Each step waits for what the one before shows, so that no key comes early.
    seen(&mut app, "@@ -2 +2 @@");
    // The first hunk's header: viewed, then staged, then each undone.
    app.send_keys("n v");
    seen(&mut app, "@@ -2 +2 @@  \u{b7} viewed");
    let modeline = app.state()["modeline"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(
        modeline.contains("0 of 2 hunks decided, 1 viewed"),
        "{modeline}"
    );
    app.send_keys("A");
    assert_eq!(
        echo_until(&mut app, "Staged 1 hunk of a.rs"),
        "Staged 1 hunk of a.rs"
    );
    assert!(
        !git_ran(&root, &["diff", "--cached", "--quiet"]),
        "a.rs is staged"
    );
    seen(&mut app, "@@ -2 +2 @@  \u{b7} accepted \u{b7} viewed");
    app.send_keys("u");
    assert_eq!(
        echo_until(&mut app, "Unstaged 1 hunk of a.rs"),
        "Unstaged 1 hunk of a.rs"
    );
    assert!(
        git_ran(&root, &["diff", "--cached", "--quiet"]),
        "nothing staged"
    );

    // A new file is not accepted.
    app.send_keys("N A");
    assert_eq!(
        app.state()["echo"],
        "A new file: M-x delete-file removes it"
    );

    // Rejected while a.rs holds an unsaved edit: asked first, then reverted and saved whole.
    app.send_keys(&format!("C-x b {} RET M-> x x x RET", typing("a.rs")));
    app.wait_for("the edit", |state| {
        state["buffer"]["name"] == "a.rs"
            && state["text"]["rows"]
                .as_array()
                .is_some_and(|rows| rows.iter().any(|row| row == "xxx"))
    });
    // A review buffer is no C-x b default: the menu shows the open review again.
    app.send_keys("C-x v r w");
    app.wait_for("the review again", |state| {
        state["buffer"]["name"] == "*review: uncommitted proj-review*"
    });
    seen(&mut app, "@@ -2 +2 @@  \u{b7} viewed");
    app.send_keys("M-< n x");
    assert_eq!(
        prompt_label(&mut app).as_deref(),
        Some("a.rs has unsaved edits; revert the hunk and save it? (y or n)")
    );
    app.send_keys("y");
    let reverted = "one\ntwo\nthree\nxxx\n";
    assert_eq!(file_until(&file, reverted), reverted);
    assert_eq!(
        echo_until(&mut app, "Reverted 1 hunk of a.rs"),
        "Reverted 1 hunk of a.rs"
    );
    app.send_keys("u");
    let back = "one\n2\nthree\nxxx\n";
    assert_eq!(file_until(&file, back), back);
}

/// Waits for a row that is `row`, failing if none shows within 5 s.
fn seen(app: &mut App, row: &str) {
    let shown = rows_until(app, row);
    assert!(
        shown.iter().any(|shown| shown == row),
        "no {row:?} in {shown:?}"
    );
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn an_edit_made_under_an_open_review_shows_with_the_cursor_kept_on_its_line() {
    let scratch = Scratch::new("review-refresh");
    let mut app = open_review(&scratch);
    let file = scratch.path().join("proj-review/a.rs");
    seen(&mut app, "+2");
    // On `+2`, a.rs's new line 2.
    app.send_keys("n C-n C-n");
    assert_eq!(cursor(&app.state()), (4, 0));
    // The agent puts a line in at the top, and no key is pressed: the review shows it.
    std::fs::write(&file, b"zero\none\n2\nthree\n").unwrap();
    seen(&mut app, "+zero");
    let shown = rows(&mut app, 20);
    let at = shown.iter().position(|row| row == "+2").expect("the line");
    assert_eq!(
        cursor(&app.state()).0,
        at as u64,
        "the cursor stays on +2: {shown:?}"
    );
    // A hunk new since the reading before says so until the cursor is in it.
    assert!(
        shown
            .iter()
            .any(|row| row.starts_with("@@ -0,0 +1 @@") && row.ends_with("changed")),
        "{shown:?}"
    );
}
