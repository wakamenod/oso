//! Asking in a review (ADR 0163), with `git_agent`'s fake `claude`: `Q` on a changed line sends
//! the question with its place and hunk, pasted and then submitted, to the review's session
//! outside a space with an orchestrator, and to the space's orchestrator in one, its terminal
//! shown in the review layout's slot while the review stays selected, below the source the slot
//! shows (ADR 0169), the split kept across a restart; and `C-c C-c`'s message goes to the
//! orchestrator too, naming the worktree.
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

/// The rule of [`boxed_screen`]'s input box, 20 columns, the fewest oso reads as a box's.
const NARROW: &str = "\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\
                      \u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}";

/// The top rule of [`boxed_screen`]'s input box, 20 columns, with the session's name at its
/// right end, as Claude Code draws it for a session started with `--name`, as oso starts them.
const NAMED: &str = "\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\
                     \u{2500} worker \u{2500}";

/// A screen with Claude Code's input box holding `input` after its sign and no-break space,
/// then the rows of `more`, its rules narrow enough for any window the flows show it in: the
/// captured screens' rules are 100 columns, which wrap in a narrower terminal, and oso reads a
/// wrapped box as none. The window below the review layout's slot, whose size the terminal
/// takes (ADR 0169), is 22 columns on the end-to-end frame.
fn boxed_screen(input: &str, more: &[&str]) -> String {
    let more: String = more.iter().map(|row| format!("{row}\n")).collect();
    format!("talk\n{NAMED}\n\u{276f}\u{a0}{input}\n{more}{NARROW}\n  ? for shortcuts\n")
}

/// Shows an empty input box in the fake's terminal, and waits until oso has it on screen.
fn empty_box(app: &mut App, flow: &Flow, terminal_name: &str) {
    flow.scratch
        .write("screen", boxed_screen("", &[]).as_bytes());
    // The sign alone, as a screen before it, a folded paste's, has the same rules.
    app.wait_for("the empty input box", |state| {
        terminal(state, terminal_name).is_some_and(|terminal| {
            (terminal["rows"].as_array()).is_some_and(|rows| {
                let has = |text: &str| rows.iter().any(|row| row.as_str() == Some(text));
                has(NARROW) && has("\u{276f}")
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
            "no {what}; echo {:?}, buffer {}; windows {}; terminals {}; state {}",
            echo(&state),
            state["buffer"]["name"],
            windows_said(&state),
            state["terminals"]
                .to_string()
                .chars()
                .take(2000)
                .collect::<String>(),
            state.to_string().chars().take(4000).collect::<String>()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Each window's id, buffer's name, area and text rows, for a failure to say.
fn windows_said(state: &Value) -> String {
    let buffers = state["buffers"].as_array().cloned().unwrap_or_default();
    let windows = state["windows"].as_array().cloned().unwrap_or_default();
    (windows.iter())
        .map(|window| {
            let name = (buffers.iter())
                .find(|buffer| buffer["id"] == window["buffer"])
                .map_or(Value::Null, |buffer| buffer["name"].clone());
            format!(
                "#{} {name} {} rows {}",
                window["id"], window["area"], window["rows"]
            )
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

/// Waits for the fake to read the question's paste, then shows the folded paste in its box,
/// as the TUI would.
fn fold_paste(app: &mut App, flow: &Flow, pastes_before: usize, terminal_name: &str) {
    let read = pastes(flow, pastes_before + 1);
    // The TUI counts the paste's line breaks, which a terminal may write as CR.
    let paste = (read.rsplit("\u{1b}[200~").next())
        .and_then(|paste| paste.split(END).next())
        .unwrap_or_default();
    let breaks = paste.matches(['\n', '\r']).count() - paste.matches("\r\n").count();
    // Wrapped at a space with the box's indent, as the TUI wraps it in a narrow terminal, so
    // that it fits the window below the slot: `[Pasted text #1 +N lines]`.
    let rest = format!("  +{breaks} lines]");
    flow.scratch.write(
        "screen",
        boxed_screen("[Pasted text #1", &[&rest]).as_bytes(),
    );
    let folded = ["\u{276f}\u{a0}[Pasted text #1", rest.as_str()];
    wait_saying(app, "the folded paste on screen", |state| {
        terminal(state, terminal_name).is_some_and(|terminal| {
            (terminal["rows"].as_array()).is_some_and(|rows| {
                (rows.windows(2)).any(|pair| pair[0] == folded[0] && pair[1] == folded[1])
            })
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

/// The frame's windows: what each shows, empty for a buffer the state does not list, as the
/// review's own, and its top row and left column.
fn placed(state: &Value) -> Vec<(String, u64, u64)> {
    let buffers = state["buffers"].as_array().cloned().unwrap_or_default();
    let name = |id: &Value| {
        (buffers.iter())
            .find(|buffer| buffer["id"] == *id)
            .and_then(|buffer| buffer["name"].as_str().map(str::to_owned))
    };
    let windows = state["windows"].as_array().cloned().unwrap_or_default();
    (windows.iter())
        .filter_map(|window| {
            let area = &window["area"];
            Some((
                name(&window["buffer"]).unwrap_or_default(),
                area["row"].as_u64()?,
                area["column"].as_u64()?,
            ))
        })
        .collect()
}

/// Waits until the frame shows `top` in the slot's upper half and `below` under it, the two
/// alone in the slot's column at the right, every other window, the review's, at its left
/// (ADR 0169), and gives the state.
fn split_slot(app: &mut App, top: &str, below: &str) -> Value {
    wait_saying(app, "the slot split, the session below", |state| {
        let placed = placed(state);
        let at = |name: &str| placed.iter().find(|(shows, ..)| shows == name);
        let (Some((_, upper, column)), Some((_, lower, under))) = (at(top), at(below)) else {
            return false;
        };
        let in_slot = (placed.iter()).filter(|(.., left)| left == column).count();
        column == under
            && upper < lower
            && in_slot == 2
            && (placed.iter()).all(|(.., left)| left <= column)
            && placed.len() > 2
    })
}

/// Waits until the window below the slot showing `session` owns its terminal's size, and the
/// PTY fits in it (ADR 0169, amending ADR 0120): the review stays selected meanwhile.
fn sized_below(app: &mut App, session: &str) {
    wait_saying(app, "the session sized to the window below", |state| {
        let buffers = state["buffers"].as_array().cloned().unwrap_or_default();
        let id = (buffers.iter()).find(|buffer| buffer["name"] == session);
        let windows = state["windows"].as_array().cloned().unwrap_or_default();
        let below = id.and_then(|id| (windows.iter()).find(|window| window["buffer"] == id["id"]));
        let (Some(below), Some(terminal)) = (below, terminal(state, session)) else {
            return false;
        };
        // The text's rows exactly; the columns at most the area's, which holds the gutter.
        let rows = terminal["size"]["rows"].as_u64();
        let columns = (terminal["size"]["columns"].as_u64())
            .zip(below["area"]["columns"].as_u64())
            .is_some_and(|(size, area)| size > 0 && size <= area);
        terminal["owner"] == below["id"]
            && rows.is_some()
            && rows == below["rows"].as_u64()
            && columns
            && state["buffer"]["name"] == REVIEW
    });
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
    // Another file than the source, so that `RET` is seen to change the slot's top.
    app.open_file(&flow.repo.join(".gitignore"));
    app.wait_for(".gitignore in its project", |state| {
        state["buffer"]["name"] == ".gitignore"
            && (state["modeline"].as_str()).is_some_and(|modeline| modeline.contains("  repo  "))
    });
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
    // The slot showed .gitignore: it stays on top, the session below it (ADR 0169), sized to
    // the window below, as the window the session started in shows .gitignore now.
    split_slot(&mut app, ".gitignore", "*claude: repo*");
    sized_below(&mut app, "*claude: repo*");

    // `RET` to the source changes the top alone, selected there.
    app.send_keys("M-< n C-n RET");
    wait_saying(&mut app, "the source on top", |state| {
        state["buffer"]["name"] == "README.md"
    });
    split_slot(&mut app, "README.md", "*claude: repo*");

    // Back in the review, a second question reuses the window below.
    app.send_keys("C-x v r");
    wait_saying(&mut app, "the review again", |state| {
        state["buffer"]["name"] == REVIEW
    });
    empty_box(&mut app, &flow, "*claude: repo*");
    ask(&mut app, "And");
    fold_paste(&mut app, &flow, 1, "*claude: repo*");
    submitted(&mut app, &flow, 2);
    let state = split_slot(&mut app, "README.md", "*claude: repo*");
    assert_eq!(state["buffer"]["name"], REVIEW, "{state}");
    flow.exit_with(0);
}

/// The split slot survives a restart: the frame comes back as the review, the source on top
/// and the session below, and a question after it reuses the window below (ADR 0169, Q3).
#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_restart_keeps_the_split_slot_and_a_question_after_it_reuses_the_window_below() {
    let flow = Flow::with_fake("review-ask-restart", "", FAKE);
    let (mut old, _bridge) = hosted(&flow);
    empty_box(&mut old, &flow, "*claude: repo*");
    back_to(&mut old, "README.md");
    show_review(&mut old);
    ask(&mut old, "Why");
    fold_paste(&mut old, &flow, 0, "*claude: repo*");
    submitted(&mut old, &flow, 1);
    split_slot(&mut old, "README.md", "*claude: repo*");

    let mut new = flow.start_for_restart();
    crate::terminal::restart::hand_over(&mut old, &mut new);
    new.wait_for("the session carried", |state| {
        terminal(state, "*claude: repo*").is_some_and(|terminal| terminal["running"] == true)
    });
    let state = split_slot(&mut new, "README.md", "*claude: repo*");
    assert_eq!(state["buffer"]["name"], REVIEW, "{state}");
    sized_below(&mut new, "*claude: repo*");
    // The review read again, as the new instance opens it.
    rows_until(&mut new, |rows| has(rows, "+changed"));

    empty_box(&mut new, &flow, "*claude: repo*");
    ask(&mut new, "And");
    fold_paste(&mut new, &flow, 1, "*claude: repo*");
    submitted(&mut new, &flow, 2);
    let state = split_slot(&mut new, "README.md", "*claude: repo*");
    assert_eq!(state["buffer"]["name"], REVIEW, "{state}");
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
