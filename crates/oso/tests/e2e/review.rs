//! Reviews in a running instance (ADR 0122): a trusted project's uncommitted changes reviewed
//! with `C-x v r w`, its review buffer read whole with each file's header, hunk and lines, `n`
//! moving to the first hunk, `g` reading a file changed meanwhile, and `q` going back; and the
//! person's comments: typed in a draft, which takes the input method's text, kept as a card,
//! edited, dropped to the kill ring, removed and yanked back, and asked about before the review
//! closes; its sides highlighted, as `global-font-lock-mode` allows; and `RET` opening a line's
//! file in another window, carried through unsaved edits, from a pane too, and `C-u RET` its old
//! side read-only.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test. The sections'
//! text, the rows and the keys have their own tests.

use std::ffi::{OsStr, OsString};
use std::path::Path;

use serde_json::{Value, json};

use super::config::set_mode;
use super::harness::{App, FAKE_SECRET, Scratch};
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
        " one",
        "@@ -2 +2 @@",
        "-two",
        "+2",
        " three",
        "added  b.rs  +1 \u{2212}0",
        "@@ -0,0 +1 @@",
        "+fn b() {}",
    ];
    assert_eq!(
        &shown[..expected.len().min(shown.len())],
        &expected[..],
        "{shown:?}"
    );

    // `n` goes to the first hunk's header, under its line of context.
    app.send_keys("n");
    assert_eq!(cursor(&app.state()), (3, 0));

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
    assert_eq!(cursor(&app.state()), (5, 0));
    app.send_keys("c");
    assert_eq!(
        prompt_label(&mut app).as_deref(),
        Some("Comment on a.rs L2 (new)")
    );
    assert_eq!(app.state()["prompts"][0]["kind"], "draft");
    // The draft takes Japanese at once, as a buffer does, from a review that takes none
    // (ADR 0158): the input method's commit goes into it.
    let state = app.state();
    assert_eq!(state["ime"]["allowed"], true, "on in the draft");
    app.call("ime", Some(json!({ "commit": "日本" })));
    assert_eq!(app.state()["prompts"][0]["text"], "日本");
    app.send_keys("DEL DEL");
    assert_eq!(app.state()["prompts"][0]["text"], "");
    app.send_keys(&format!(
        "{} RET {} C-c C-c",
        typing("first"),
        typing("second")
    ));
    assert_eq!(prompt_label(&mut app), None, "the draft is kept and closed");
    assert_eq!(
        app.state()["ime"]["allowed"],
        false,
        "off in the review again"
    );
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
    assert_eq!(cursor(&app.state()), (6, 0));
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
    assert_eq!(cursor(&app.state()), (6, 0));

    // `k` removes it at once, and `C-y` yanks it back as text elsewhere.
    app.send_keys("k");
    assert_eq!(app.state()["echo"], "Removed #0: C-y yanks it");
    gone(&mut app, "You  #0");
    // `q` puts back the windows from before the review layout once the key ends, so the next
    // keys are sent as the person types them, after it (ADR 0160).
    app.send_keys("q");
    app.wait_for("a.rs", |state| state["buffer"]["name"] == "a.rs");
    app.send_keys("M-> C-y");
    let state = app.state();
    assert_eq!(state["buffer"]["name"], "a.rs");
    let text = rows(&mut app, 20).join("\n");
    assert!(text.ends_with("You  #0\nfirst\nsecond"), "{text:?}");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_draft_takes_no_japanese_once_it_or_a_buffer_shown_may_hold_secrets() {
    let scratch = Scratch::new("review-draft-secret");
    let mut app = open_review(&scratch);
    let review = app.state()["buffer"]["name"].clone();
    // A buffer that may hold secrets, outside the project, in the review layout's slot, where a
    // line of it is killed.
    let env = scratch.write(".env", format!("{FAKE_SECRET}\n").as_bytes());
    app.send_keys("C-x o");
    app.open_file(&env);
    assert_eq!(app.wait_until_ready()["buffer"]["secret"], true);
    app.send_keys("C-k C-x o");
    assert_eq!(app.state()["buffer"]["name"], review);

    // Shown in another window, it keeps the IME off in a draft (ADR 0158).
    app.send_keys("M-< n C-n C-n c");
    let state = app.state();
    assert_eq!(state["prompts"][0]["kind"], "draft");
    assert_eq!(state["ime"]["allowed"], false, "a secret buffer is shown");
    app.send_keys("C-g");
    assert_eq!(prompt_label(&mut app), None);

    // With it gone, the draft takes Japanese; its kill yanked in turns the draft secret, and
    // the IME goes off, and oso's own layer gives no input context, so no text service reads
    // the view; winit none.
    app.send_keys("C-x 1");
    assert_eq!(app.state()["buffer"]["name"], review);
    app.send_keys("c");
    assert_eq!(app.state()["prompts"][0]["kind"], "draft");
    assert_eq!(app.state()["ime"]["allowed"], true, "on in the draft");
    app.send_keys("C-y");
    assert_eq!(
        app.state()["ime"]["allowed"],
        false,
        "off in a secret draft"
    );
    let none = if cfg!(target_os = "macos") {
        json!(false)
    } else {
        Value::Null
    };
    app.wait_for("no input context", |state| {
        state["ime"]["input_context"] == none
    });
    app.send_keys("C-g");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_restart_opens_the_review_again_with_its_comment_in_its_window() {
    let scratch = Scratch::new("review-restart");
    let mut old = open_review(&scratch);
    old.send_keys("n C-n C-n c");
    old.send_keys(&format!("{} C-c C-c", typing("kept")));
    rows_until(&mut old, "    \u{258d}  kept");
    // The new instance shares the flow's state directory, as a restart's two do.
    let config = scratch.path().join("config.toml");
    let mut new = App::start_with_config(None, scratch.path(), &config);
    new.wait_until_ready();
    let _ = super::handover::hand_over(&mut old, &mut new);
    new.wait_for("the review again", |state| {
        state["buffer"]["name"] == "*review: uncommitted proj-review*"
    });
    let shown = rows_until(&mut new, "    \u{258d}  kept");
    assert!(
        shown.iter().any(|row| row == "    \u{258d}You  #0"),
        "the comment, by its id: {shown:?}"
    );
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
    assert_eq!(cursor(&app.state()), (5, 0));

    // `|`: the new side's pane holds the cursor, beside `-two` in the old one. The pair takes
    // the review layout's left, the slot staying at its right (ADR 0160, Q5).
    app.send_keys("|");
    let state = app.wait_for("the panes", |state| {
        state["buffer"]["name"] == "*review: uncommitted proj-review \u{b7} new*"
    });
    assert_eq!(cursor(&state), (4, 0));
    assert_eq!(window_count(&mut app), 3);
    let new = rows_until(&mut app, "+fn b() {}");
    assert_eq!(new[4], "+2", "{new:?}");
    // Past the slot, to the old pane.
    app.send_keys("C-x o C-x o");
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
    assert_eq!(old[4], "-two", "{old:?}");
    // Context goes in both panes, row for row.
    assert_eq!((old[2].as_str(), new[2].as_str()), (" one", " one"));
    assert_eq!((old[5].as_str(), new[5].as_str()), (" three", " three"));
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
    assert_eq!(cursor(&state), (5, 0));
    assert_eq!(window_count(&mut app), 2);

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
    assert_eq!(window_count(&mut app), 2);
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
    // A review buffer is no C-x b default: the menu shows the open review again, which
    // `C-u C-x v r` opens while a review is open (ADR 0160).
    app.send_keys("C-u C-x v r w");
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

/// Waits until no row holds `part`, failing if one still does after 5 s.
fn gone(app: &mut App, part: &str) {
    let mut shown = Vec::new();
    for _ in 0..200 {
        shown = rows(app, 20);
        if !shown.iter().any(|row| row.contains(part)) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    panic!("{part:?} still in {shown:?}");
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
    assert_eq!(cursor(&app.state()), (5, 0));
    // The agent puts a line in at the top, and no key is pressed: the review shows it.
    std::fs::write(&file, b"zero\none\n2\nthree\n").unwrap();
    // A hunk new since the reading before says so until the cursor is in it. The text after the
    // one that first shows the hunk says it, as the model has the hunk only once that is in.
    const CHANGED: &str = "@@ -0,0 +1 @@  \u{b7} changed";
    seen(&mut app, CHANGED);
    let shown = rows(&mut app, 20);
    let header = shown.iter().position(|row| row == CHANGED).expect("it");
    assert_eq!(shown[header + 1], "+zero", "its hunk: {shown:?}");
    let at = shown.iter().position(|row| row == "+2").expect("the line");
    assert_eq!(
        cursor(&app.state()).0,
        at as u64,
        "the cursor stays on +2: {shown:?}"
    );
}

/// Whether the current buffer shows a run of a syntax class.
fn syntax(state: &Value) -> bool {
    state["buffer"]["syntax"] == true
}

/// A review's sides are highlighted once read (ADR 0122, step 9), in its panes too, and
/// `global-font-lock-mode` turns that off and on, as it does a buffer's. Which line takes which run is the view's
/// unit tests'.
#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_reviews_sides_are_highlighted_and_font_lock_turns_that_off_and_on() {
    let scratch = Scratch::new("review-highlight");
    let mut app = open_review(&scratch);
    app.wait_for("the sides highlighted", syntax);
    app.run_command("global-font-lock-mode", None).unwrap();
    app.wait_for("the sides plain", |state| !syntax(state));
    app.run_command("global-font-lock-mode", None).unwrap();
    app.wait_for("the sides highlighted again", syntax);
    assert_eq!(
        app.state()["buffer"]["name"],
        "*review: uncommitted proj-review*"
    );
    // `|`: the new side's pane is highlighted too.
    app.send_keys("|");
    app.wait_for("the new pane highlighted", |state| {
        state["buffer"]["name"] == "*review: uncommitted proj-review \u{b7} new*" && syntax(state)
    });
    app.send_keys("|");
}

/// The first window of `state` not showing `buffer`.
fn window_not_showing(state: &Value, buffer: &Value) -> Value {
    let windows = state["windows"].as_array().cloned().unwrap_or_default();
    (windows.into_iter())
        .find(|window| window["buffer"] != *buffer)
        .unwrap_or_default()
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn the_files_pane_lists_and_shows_files_and_the_filter_hides_them_across_a_restart() {
    let scratch = Scratch::new("review-files");
    let mut app = open_review(&scratch);
    const PANE: &str = "*review: uncommitted proj-review \u{b7} files*";
    const HIDDEN: &str = "1 of 2 files hidden by \"b.rs\" \u{b7} / to change";

    // `s`: the pane at the review's left, selected, a row a file under its title; the review
    // layout's slot stays at the right (ADR 0160, Q6).
    app.send_keys("s");
    app.wait_for("the pane", |state| state["buffer"]["name"] == PANE);
    assert_eq!(window_count(&mut app), 3);
    let pane = app.state()["buffer"]["id"].clone();
    let listed = rows_until(&mut app, "2 files \u{b7} / to filter");
    assert!(
        listed.iter().any(|row| row.ends_with("  a.rs"))
            && listed.iter().any(|row| row.ends_with("  b.rs")),
        "{listed:?}"
    );

    // `n n SPC`: b.rs's header at the review's top, the pane still selected; `RET` selects it.
    app.send_keys("n n SPC");
    let state = app.state();
    assert_eq!(state["buffer"]["name"], PANE);
    let review = window_not_showing(&state, &pane);
    assert_eq!(review["scroll"]["start"], review["selections"][0]["cursor"]);
    app.send_keys("RET");
    let state = app.wait_for("the review selected", |state| {
        state["buffer"]["name"] == "*review: uncommitted proj-review*"
    });
    let row = usize::try_from(cursor(&state).0).unwrap();
    let shown = rows(&mut app, row + 1);
    assert!(shown[row].contains("b.rs"), "on b.rs's header: {shown:?}");

    // `/`: only b.rs, the title saying what is hidden, the pane too.
    app.send_keys(&format!("/ {} RET", typing("b.rs")));
    let shown = rows_until(&mut app, HIDDEN);
    assert!(shown.iter().any(|row| row == HIDDEN), "{shown:?}");
    assert!(!shown.iter().any(|row| row.contains("a.rs")), "{shown:?}");
    // `/` again opens holding the filter.
    app.send_keys("/");
    let prompts = app.state()["prompts"].clone();
    assert_eq!(prompts[0]["text"], "b.rs", "{prompts}");
    app.send_keys("C-g");

    // A restart: the review, its filter and its pane come back.
    let config = scratch.path().join("config.toml");
    let mut new = App::start_with_config(None, scratch.path(), &config);
    new.wait_until_ready();
    let _ = super::handover::hand_over(&mut app, &mut new);
    new.wait_for("the review again", |state| {
        state["buffer"]["name"] == "*review: uncommitted proj-review*"
    });
    let shown = rows_until(&mut new, HIDDEN);
    assert!(shown.iter().any(|row| row == HIDDEN), "{shown:?}");
    new.wait_for("the pane again", |state| {
        state["windows"].as_array().map_or(0, Vec::len) == 3
    });
    new.send_keys("s");
    new.wait_for("the pane selected", |state| state["buffer"]["name"] == PANE);
    let listed: Vec<String> = (rows_until(&mut new, HIDDEN).into_iter())
        .filter(|row| !row.is_empty())
        .collect();
    assert_eq!(listed.len(), 2, "the title and b.rs: {listed:?}");
    assert!(listed[1].ends_with("  b.rs"), "{listed:?}");
    // `q`: the pane goes, its columns back to the review.
    new.send_keys("q");
    new.wait_for("the pane gone", |state| {
        state["windows"].as_array().map_or(0, Vec::len) == 2
    });
}

/// The current buffer's read-only reason, if any.
fn read_only(app: &mut App) -> Value {
    app.state()["buffer"]["read_only"].clone()
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn ret_opens_the_line_carried_through_unsaved_edits_and_c_u_ret_the_old_side() {
    let scratch = Scratch::new("review-visit");
    set_mode(scratch.path(), 0o700);
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
    app.send_keys(&format!("M-x {} RET y", typing("trust-project")));
    app.wait_for("the trust", |state| {
        state["echo"]
            .as_str()
            .is_some_and(|echo| echo.starts_with("Trusted "))
    });
    // A line put in at a.rs's top in its buffer, not saved: the review reads the file.
    app.send_keys(&format!("M-< {} RET", typing("zero")));
    app.send_keys("C-x v r w");
    app.wait_for("the review", |state| {
        state["buffer"]["name"] == "*review: uncommitted proj-review*"
    });
    rows_until(&mut app, "+fn b() {}");

    // `RET` on `+2`, new line 2: a.rs in another window, at line 3 of its buffer now.
    app.send_keys("n C-n C-n RET");
    let state = app.wait_for("a.rs", |state| state["buffer"]["name"] == "a.rs");
    assert_eq!(cursor(&state), (2, 0));
    assert_eq!(window_count(&mut app), 2);
    let review = window_not_showing(&state, &state["buffer"]["id"]);
    assert!(!review.is_null(), "the review stays in its window: {state}");

    // `C-u RET` on `-two`: the old side, read-only, at its line 2.
    app.send_keys("C-x o C-p C-u RET");
    let state = app.wait_for("the old side", |state| {
        state["buffer"]["name"] == "*old: a.rs \u{b7} uncommitted*"
    });
    assert_eq!(cursor(&state), (1, 0));
    assert_eq!(rows(&mut app, 3), ["one", "two", "three"]);
    assert!(!read_only(&mut app).is_null(), "{state}");
    assert_eq!(window_count(&mut app), 2);

    // In the panes, the cursor on `-two` is in the old one, whose `RET` goes to where the line
    // was removed, carried as before, in a window outside the pair.
    app.send_keys("C-x o |");
    app.wait_for("the panes", |state| {
        state["buffer"]["name"] == "*review: uncommitted proj-review \u{b7} old*"
    });
    assert_eq!(window_count(&mut app), 3);
    app.send_keys("RET");
    let state = app.wait_for("a.rs again", |state| state["buffer"]["name"] == "a.rs");
    assert_eq!(cursor(&state), (2, 0));
    assert_eq!(window_count(&mut app), 3, "the pair stays: {state}");
}

/// Each window of `state` as the review layout's restore must give it back: its name, its
/// buffer, its area and its cursors.
fn window_places(state: &Value) -> Vec<Value> {
    let windows = state["windows"].as_array().cloned().unwrap_or_default();
    (windows.into_iter())
        .map(|window| {
            json!([
                window["id"],
                window["buffer"],
                window["area"],
                window["selections"]
            ])
        })
        .collect()
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn the_review_layout_keeps_one_slot_goes_back_to_the_review_and_restores_the_windows() {
    let scratch = Scratch::new("review-layout");
    set_mode(scratch.path(), 0o700);
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
    app.send_keys(&format!("M-x {} RET y", typing("trust-project")));
    app.wait_for("the trust", |state| {
        state["echo"]
            .as_str()
            .is_some_and(|echo| echo.starts_with("Trusted "))
    });
    // The person's windows: a.rs twice, stacked, the lower one at its end.
    app.send_keys("C-x 2 C-x o M-> C-x o");
    let before = window_places(&app.state());
    assert_eq!(before.len(), 2);

    // `C-x v r` with no review open opens the menu; `w` opens the review in the layout: the
    // review at the left, the slot at the right showing a.rs.
    app.send_keys("C-x v r");
    assert_eq!(app.state()["menu"], "review");
    app.send_keys("w");
    let state = app.wait_for("the review", |state| {
        state["buffer"]["name"] == "*review: uncommitted proj-review*"
    });
    let review = state["buffer"]["id"].clone();
    assert_eq!(window_count(&mut app), 2);
    rows_until(&mut app, "+fn b() {}");

    // `RET` on `+2`: a.rs in the slot, still two windows.
    app.send_keys("n C-n C-n RET");
    app.wait_for("a.rs", |state| state["buffer"]["name"] == "a.rs");
    assert_eq!(window_count(&mut app), 2);
    // `C-x v r` in the slot: back to the review, on `+2`.
    app.send_keys("C-x v r");
    let state = app.state();
    assert_eq!(state["buffer"]["id"], review);
    assert_eq!(cursor(&state), (5, 0));

    // `C-u RET` on `-two`: the old side in the slot, in place of a.rs.
    app.send_keys("C-p C-u RET");
    app.wait_for("the old side", |state| {
        state["buffer"]["name"] == "*old: a.rs \u{b7} uncommitted*"
    });
    assert_eq!(window_count(&mut app), 2);
    app.send_keys("C-x v r");
    let state = app.state();
    assert_eq!(state["buffer"]["id"], review);
    assert_eq!(cursor(&state), (4, 0));

    // `C-x v r` in the review: the windows from before, exactly.
    app.send_keys("C-x v r");
    let state = app.state();
    assert_eq!(window_places(&state), before);
    assert_eq!(state["buffer"]["name"], "a.rs");

    // Again, and `q` restores too.
    app.send_keys("C-x v r");
    app.wait_for("the layout again", |state| state["buffer"]["id"] == review);
    assert_eq!(window_count(&mut app), 2);
    app.send_keys("q");
    assert_eq!(window_places(&app.state()), before);
}

/// A review of `proj-review`'s uncommitted changes, its `a.txt` of 30 lines with line 15
/// changed, open, with the project trusted.
fn open_context_review(scratch: &Scratch) -> App {
    set_mode(scratch.path(), 0o700);
    let config = scratch.write("config.toml", b"[lsp]\nstart = \"manual\"\n");
    set_mode(&config, 0o644);
    let numbered = |changed: u32| -> String {
        (1..=30)
            .map(|n| match n == changed {
                true => format!("L{n}\n"),
                false => format!("l{n}\n"),
            })
            .collect()
    };
    let file = scratch.write("proj-review/a.txt", numbered(0).as_bytes());
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
    std::fs::write(&file, numbered(15)).unwrap();
    let mut app = App::start_with_config(Some(&file), scratch.path(), &config);
    app.wait_until_ready();
    // The project is found off the UI thread: trusting it before then finds no project.
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
    app
}

/// Unchanged lines around a hunk (step 13): three each side, gaps' rows for the rest, `[` and
/// `]` showing a gap's lines, `review-reset-context` hiding them again, `RET` on a context line
/// visiting it, and `c` refusing it.
#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn context_lines_show_around_a_hunk_and_the_context_keys_show_more() {
    let scratch = Scratch::new("review-context");
    let mut app = open_context_review(&scratch);
    let shown = rows_until(&mut app, "\u{22ef} 12 unchanged lines");
    let expected = [
        "Review \u{b7} uncommitted \u{b7} proj-review",
        "modified  a.txt  +1 \u{2212}1",
        "\u{22ef} 11 unchanged lines",
        " l12",
        " l13",
        " l14",
        "@@ -15 +15 @@",
        "-l15",
        "+L15",
        " l16",
        " l17",
        " l18",
        "\u{22ef} 12 unchanged lines",
    ];
    assert_eq!(
        &shown[..expected.len().min(shown.len())],
        &expected[..],
        "{shown:?}"
    );

    // `c` on a context line refuses; `RET` visits it.
    app.send_keys("n C-p c");
    assert_eq!(
        app.state()["echo"],
        "Comments go on a changed line or a hunk's header"
    );
    assert_eq!(prompt_label(&mut app), None);
    app.send_keys("RET");
    let state = app.wait_for("a.txt", |state| state["buffer"]["name"] == "a.txt");
    assert_eq!(cursor(&state), (13, 0), "on line 14");

    // `[` above the hunk shows the 11 hidden lines, the gap being no more than 20; `]` the 12
    // below. The cursor stays on the hunk's header.
    app.send_keys("C-x o n [");
    seen(&mut app, " l1");
    app.send_keys("]");
    let shown = rows_until(&mut app, " l30");
    assert!(
        !shown.iter().any(|row| row.starts_with('\u{22ef}')),
        "{shown:?}"
    );
    let header = shown.iter().position(|row| row == "@@ -15 +15 @@").unwrap();
    assert_eq!(cursor(&app.state()).0, header as u64);

    // A refresh keeps them; `review-reset-context` hides them again.
    app.send_keys("g");
    assert_eq!(rows_until(&mut app, " l30")[2], " l1");
    app.send_keys(&format!("M-x {} RET", typing("review-reset-context")));
    seen(&mut app, "\u{22ef} 11 unchanged lines");
}
