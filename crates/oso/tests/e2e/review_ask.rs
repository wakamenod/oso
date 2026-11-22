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
    wait_saying(app, "the review", |state| state["buffer"]["name"] == REVIEW);
    rows_until(app, |rows| has(rows, "+changed"));
}

/// [`show_review`] in a space whose orchestrator is recorded, where the menu first asks what
/// of the space's repository to review (ADR 0163): this checkout, then its uncommitted changes.
fn show_review_in_space(app: &mut App) {
    app.send_keys("C-u C-x v r");
    wait_saying(app, "review-start's menu", |state| {
        state["menu"] == "review-start"
    });
    // The review menu opens from the host once `.` ran, so `w` waits for it.
    app.send_keys(".");
    wait_saying(app, "the review menu", |state| state["menu"] == "review");
    app.send_keys("w");
    wait_saying(app, "the review", |state| state["buffer"]["name"] == REVIEW);
    rows_until(app, |rows| has(rows, "+changed"));
}

/// The rule of [`boxed_screen`]'s input box, 30 columns.
const NARROW: &str = "\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\
                      \u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\
                      \u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}";

/// The top rule of [`boxed_screen`]'s input box, 30 columns, with the session's name at its
/// right end, as Claude Code draws it for a session started with `--name`, as oso starts them.
const NAMED: &str = "\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\
                     \u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\
                     \u{2500} worker \u{2500}";

/// A screen with Claude Code's input box holding `input` after its sign and no-break space,
/// its rules narrow enough for any window the flows show it in: the captured screens' rules
/// are 100 columns, which wrap in a narrower terminal, and oso reads a wrapped box as none.
fn boxed_screen(input: &str) -> String {
    format!("talk\n{NAMED}\n\u{276f}\u{a0}{input}\n{NARROW}\n  ? for shortcuts\n")
}

/// Shows an empty input box in the fake's terminal, and waits until oso has it on screen.
fn empty_box(app: &mut App, flow: &Flow, terminal_name: &str) {
    box_holding(app, flow, terminal_name, "");
}

/// Shows the input box holding `input` in the fake's terminal, and waits until oso has it on
/// screen.
fn box_holding(app: &mut App, flow: &Flow, terminal_name: &str, input: &str) {
    flow.scratch.write("screen", boxed_screen(input).as_bytes());
    let sign = format!("\u{276f}\u{a0}{input}");
    let sign = sign.trim_end();
    app.wait_for("the input box", |state| {
        terminal(state, terminal_name).is_some_and(|terminal| {
            (terminal["rows"].as_array()).is_some_and(|rows| {
                rows.iter().any(|row| row.as_str() == Some(NARROW))
                    && rows.iter().any(|row| row.as_str() == Some(sign))
            })
        })
    });
}

/// Waits until `holds` holds of the state, saying the echo area and the buffer if it never
/// does.
fn wait_saying(app: &mut App, what: &str, holds: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let state = app.state();
        if holds(&state) {
            return state;
        }
        assert!(
            Instant::now() < deadline,
            "no {what}; echo {:?}, buffer {}; state {}",
            echo(&state),
            state["buffer"]["name"],
            state.to_string().chars().take(4000).collect::<String>()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Waits for the fake to read the question's paste, then shows the folded paste in its box,
/// as the TUI would: the Enter waits for a screen newer than the paste.
fn fold_paste(app: &mut App, flow: &Flow, pastes_before: usize, terminal_name: &str) {
    let read = pastes(flow, pastes_before + 1);
    // The TUI counts the paste's line breaks, which a terminal may write as CR.
    let paste = (read.rsplit("\u{1b}[200~").next())
        .and_then(|paste| paste.split(END).next())
        .unwrap_or_default();
    let breaks = paste.matches(['\n', '\r']).count() - paste.matches("\r\n").count();
    let folding = format!("[Pasted text #1 +{breaks} lines]");
    flow.scratch
        .write("screen", boxed_screen(&folding).as_bytes());
    let folded = format!("\u{276f}\u{a0}{folding}");
    wait_saying(app, "the folded paste on screen", |state| {
        terminal(state, terminal_name).is_some_and(|terminal| {
            (terminal["rows"].as_array())
                .is_some_and(|rows| rows.iter().any(|row| row.as_str() == Some(folded.as_str())))
        })
    });
}

/// What the fake has read once its input ends with a submitted paste, the `pastes`th.
fn submitted(app: &mut App, flow: &Flow, pastes: usize) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let read = fs::read_to_string(flow.scratch.path().join("input")).unwrap_or_default();
        if read.matches(END).count() >= pastes && read.ends_with(&format!("{END}\r")) {
            return read;
        }
        if Instant::now() >= deadline {
            let state = app.state();
            let waiting = flow.scratch.path().join("screen").exists();
            panic!(
                "{pastes} submitted, read {read:?}; screen file still waiting: {waiting}; echo \
                 {:?}; state {}",
                echo(&state),
                state.to_string().chars().take(4000).collect::<String>()
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The names of the buffers the frame's windows show.
fn shown(state: &Value) -> Vec<String> {
    let buffers = state["buffers"].as_array().cloned().unwrap_or_default();
    let name = |id: &Value| {
        (buffers.iter())
            .find(|buffer| buffer["id"] == *id)
            .and_then(|buffer| buffer["name"].as_str().map(str::to_owned))
    };
    let windows = state["windows"].as_array().cloned().unwrap_or_default();
    windows
        .iter()
        .filter_map(|window| name(&window["buffer"]))
        .collect()
}

/// Asks `question` with `Q` on the review's first added line.
fn ask(app: &mut App, question: &str) {
    app.send_keys("M-< n C-n Q");
    app.wait_for("the question's prompt", |state| {
        super::worktrees::asks(state).is_some_and(|prompt| prompt.starts_with("Ask about this"))
    });
    app.send_keys(&format!("{} RET", typing(question)));
    wait_saying(app, "the question asked", |state| {
        echo(state).starts_with("Asked ")
    });
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_question_goes_to_the_review_s_session_submitted_and_shows_beside_the_review() {
    let flow = Flow::with_fake("review-ask", "", FAKE);
    let (mut app, _bridge) = hosted(&flow);
    empty_box(&mut app, &flow, "*claude: repo*");
    back_to(&mut app, "README.md");
    show_review(&mut app);

    ask(&mut app, "Why");
    fold_paste(&mut app, &flow, 0, "*claude: repo*");
    let read = submitted(&mut app, &flow, 1);
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
fn a_draft_in_the_box_goes_with_the_question_and_the_echo_says_so() {
    let flow = Flow::with_fake("review-ask-draft", "", FAKE);
    let (mut app, _bridge) = hosted(&flow);
    box_holding(&mut app, &flow, "*claude: repo*", "my draft");
    back_to(&mut app, "README.md");
    show_review(&mut app);

    ask(&mut app, "Why");
    fold_paste(&mut app, &flow, 0, "*claude: repo*");
    let read = submitted(&mut app, &flow, 1);
    assert!(read.contains("Question: Why"), "{read:?}");
    let state = app.wait_for("the echo", |state| echo(state).starts_with("Asked repo"));
    assert_eq!(
        echo(&state),
        "Asked repo: its answer shows beside the review; what its input box held went with it"
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
    empty_box(&mut app, &flow, orchestrator);
    app.open_file(&flow.repo.join("README.md"));
    // Its project is found off the UI thread; the review menu needs it.
    wait_saying(&mut app, "README.md in its project", |state| {
        state["buffer"]["name"] == "README.md"
            && state["modeline"]
                .as_str()
                .is_some_and(|line| line.contains("  repo  "))
    });
    show_review_in_space(&mut app);

    // `Q` goes to the orchestrator, naming the worktree, and its terminal shows in the slot.
    ask(&mut app, "Why");
    fold_paste(&mut app, &flow, 0, orchestrator);
    let read = submitted(&mut app, &flow, 1);
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
