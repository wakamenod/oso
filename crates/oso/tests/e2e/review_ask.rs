//! Asking in a review (ADR 0163), with `git_agent`'s fake `claude`: `Q` on a changed line sends
//! the question with its place and hunk, pasted and then submitted, to the review's session
//! outside a space with an orchestrator, and to the space's orchestrator in one, its terminal
//! shown in the review layout's slot while the review stays selected; and `C-c C-c`'s message
//! goes to the orchestrator too, naming the worktree.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test: the question's
//! text, a region's rows and the message's head have their own tests.

use std::fs;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::git_agent::{END, FAKE, WORKING, back_to, echo, hosted, pastes};
use super::git_status::{has, rows_until};
use super::harness::App;
use super::review::rows;
use super::spaces::{PrivateRoot, answer, space};
use super::typing;
use super::worktrees::{Flow, terminal};

const REVIEW: &str = "*review: uncommitted repo*";

/// Shows the review of the uncommitted changes from the menu (ADR 0160), and waits for its
/// added line.
fn show_review(app: &mut App) {
    app.send_keys("C-u C-x v r w");
    app.wait_for("the review", |state| state["buffer"]["name"] == REVIEW);
    rows_until(app, |rows| has(rows, "+changed"));
}

/// The working screen with Claude Code's folded paste in its input box, as the TUI shows a
/// paste of several lines: oso submits a question only once its box shows that alone.
fn pasted_screen() -> String {
    let lines: Vec<&str> = WORKING.lines().collect();
    let at = lines
        .iter()
        .rposition(|line| line.trim_end() == "\u{276f}")
        .expect("the working screen's empty input box");
    let mut lines: Vec<String> = lines.iter().map(|line| (*line).to_owned()).collect();
    lines[at] = "\u{276f} [Pasted text #1 +12 lines]".to_owned();
    lines.join("\n") + "\n"
}

/// Waits for the fake to read the question's paste, then shows the folded paste in its box,
/// as the TUI would.
fn fold_paste(flow: &Flow, pastes_before: usize) {
    pastes(flow, pastes_before + 1);
    flow.scratch.write("screen", pasted_screen().as_bytes());
}

/// What the fake has read once its input ends with a submitted paste, the `pastes`th.
fn submitted(flow: &Flow, pastes: usize) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let read = fs::read_to_string(flow.scratch.path().join("input")).unwrap_or_default();
        if read.matches(END).count() >= pastes && read.ends_with(&format!("{END}\r")) {
            return read;
        }
        assert!(
            Instant::now() < deadline,
            "{pastes} submitted, read {read:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The names of the buffers the frame's windows show.
fn shown(state: &Value) -> Vec<String> {
    let windows = state["windows"].as_array().cloned().unwrap_or_default();
    let names = windows
        .iter()
        .filter_map(|window| window["buffer"].as_str());
    names.map(str::to_owned).collect()
}

/// Asks `question` with `Q` on the review's first added line.
fn ask(app: &mut App, question: &str) {
    app.send_keys("M-< n C-n Q");
    app.wait_for("the question's prompt", |state| {
        super::worktrees::asks(state).is_some_and(|prompt| prompt.starts_with("Ask about this"))
    });
    app.send_keys(&format!("{} RET", typing(question)));
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_question_goes_to_the_review_s_session_submitted_and_shows_beside_the_review() {
    let flow = Flow::with_fake("review-ask", "", FAKE);
    let (mut app, _bridge) = hosted(&flow);
    back_to(&mut app, "README.md");
    show_review(&mut app);

    ask(&mut app, "Why");
    let read = submitted(&flow, 1);
    assert!(
        read.contains("A question from the person, asked in a review."),
        "{read:?}"
    );
    assert!(read.contains("## README.md  L2 (new)"), "{read:?}");
    assert!(read.contains("+changed"), "{read:?}");
    assert!(read.contains("Question: Why"), "{read:?}");

    // The review stays selected; the session shows in the slot beside it.
    let state = app.wait_for("the answer beside the review", |state| {
        echo(state).starts_with("Asked repo")
    });
    assert_eq!(state["buffer"]["name"], REVIEW, "{state}");
    assert_eq!(
        echo(&state),
        "Asked repo: its answer shows beside the review"
    );
    let windows = shown(&state);
    assert!(
        windows.iter().any(|name| name == "*claude: repo*"),
        "{windows:?}"
    );
    flow.exit_with(0);
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn in_a_space_questions_and_comments_go_to_its_orchestrator() {
    let spaces = PrivateRoot::new("review-ask-space");
    let config = format!("\n[spaces]\nroot = \"{}\"\n", spaces.0.display());
    let flow = Flow::with_fake("review-ask-space", &config, FAKE);
    for (name, value) in [
        ("user.name", "e2e"),
        ("user.email", "e2e@example.invalid"),
        ("commit.gpgsign", "false"),
    ] {
        flow.git(&flow.repo, &["config", name, value]);
    }
    fs::write(flow.repo.join("README.md"), "# e2e\nchanged\n").unwrap();
    let (mut app, _) = flow.start();
    app.send_keys("C-c a O");
    answer(&mut app, "Describe the space:", "Fix login");
    let orchestrator = "*claude: fix-login*";
    app.wait_for("the orchestrator, ready and recorded", |state| {
        space(state, "fix-login").is_some_and(|s| s["orchestrator"].is_string())
            && terminal(state, orchestrator).is_some_and(|terminal| {
                (terminal["rows"].as_array())
                    .is_some_and(|rows| rows.iter().any(|row| row == "fake ready"))
            })
    });
    // Judged working, so that it takes a paste.
    flow.scratch.write("screen", WORKING.as_bytes());
    app.wait_for("judged working", |state| {
        (state["sessions"]["sessions"].as_array()).is_some_and(|rows| {
            rows.iter()
                .any(|row| row["name"] == "fix-login" && row["state"] == "working")
        })
    });
    app.open_file(&flow.repo.join("README.md"));
    show_review(&mut app);

    // `Q` goes to the orchestrator, naming the worktree, and its terminal shows in the slot.
    ask(&mut app, "Why");
    fold_paste(&flow, 0);
    let read = submitted(&flow, 1);
    let root = fs::canonicalize(&flow.repo).unwrap();
    assert!(
        read.contains(&format!("Worktree: {}.", root.display()))
            || read.contains(&format!("Worktree: {}.", flow.repo.display())),
        "{read:?}"
    );
    assert!(read.contains("Question: Why"), "{read:?}");
    let state = app.wait_for("asked", |state| echo(state).starts_with("Asked fix-login"));
    assert_eq!(state["buffer"]["name"], REVIEW, "{state}");
    assert!(
        shown(&state).iter().any(|name| name == orchestrator),
        "{:?}",
        shown(&state)
    );

    // A comment's message goes to the orchestrator too, under a line naming the worktree.
    app.send_keys("M-< n C-n c");
    app.send_keys(&format!("{} C-c C-c", typing("Rename")));
    rows_until(&mut app, |rows| has(rows, "    \u{258d}  Rename"));
    app.send_keys("C-c C-c");
    let state = app.wait_for("the message", |state| {
        state["buffer"]["name"] == "*review message*"
    });
    assert!(
        echo(&state).starts_with("1 comment to fix-login."),
        "{}",
        echo(&state)
    );
    let head = rows(&mut app, 2);
    assert!(
        head[0].starts_with("Review comments on the changes in worktree "),
        "{head:?}"
    );
    app.send_keys("C-c C-c");
    let read = pastes(&flow, 2);
    assert!(read.contains("Comment: Rename"), "{read:?}");
    assert!(
        read.ends_with(END),
        "the message is pasted, not submitted: {read:?}"
    );
    flow.exit_with(0);
}
