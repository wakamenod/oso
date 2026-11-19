//! Sending a review's comments to the worktree's agent session (ADR 0122, step 12), with
//! `git_agent`'s fake `claude`: a comment kept on a line, `C-c C-c` in the review showing
//! `*review message*` with its place, hunk and text, `C-c C-c` there pasting it into the
//! session, not submitted, and the comment then sent, so that a second `C-c C-c` has nothing
//! to gather; and a message dropped with `C-c C-k`.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test: the message's
//! text, the order of its comments and what counts as changed have their own tests.

use super::git_agent::{END, FAKE, back_to, echo, hosted, pastes};
use super::git_status::{has, rows_until};
use super::harness::App;
use super::review::rows;
use super::typing;
use super::worktrees::Flow;

const REVIEW: &str = "*review: uncommitted repo*";
const MESSAGE: &str = "*review message*";

/// Shows the review of the uncommitted changes, opening it or showing the one open, from the
/// menu `C-u C-x v r` opens whether a review is open or not (ADR 0160).
fn show_review(app: &mut App) {
    app.send_keys("C-u C-x v r w");
    app.wait_for("the review", |state| state["buffer"]["name"] == REVIEW);
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_comment_goes_to_the_session_through_its_message_and_is_then_sent() {
    let flow = Flow::with_fake("review-send", "", FAKE);
    // `hosted` changes the README, which the review shows.
    let (mut app, _bridge) = hosted(&flow);
    back_to(&mut app, "README.md");
    show_review(&mut app);
    rows_until(&mut app, |rows| has(rows, "+changed"));

    // Nothing of the person's waits yet.
    app.send_keys("C-c C-c");
    assert_eq!(echo(&app.state()), "No comment of yours waits to be sent");

    // A comment on the added line.
    app.send_keys("n C-n");
    app.send_keys("c");
    app.send_keys(&format!("{} SPC {} C-c C-c", typing("Say"), typing("why")));
    rows_until(&mut app, |rows| has(rows, "    \u{258d}  Say why"));

    // `C-c C-c` shows the message, naming the session it goes to.
    app.send_keys("C-c C-c");
    let state = app.wait_for("the message", |state| state["buffer"]["name"] == MESSAGE);
    let said = echo(&state);
    assert!(
        said.starts_with("1 comment to repo. C-c C-c pastes it"),
        "{said}"
    );
    let shown = rows(&mut app, 12);
    assert_eq!(
        shown[0],
        "Review comments on the changes below. Please act on each of them."
    );
    assert!(has(&shown, "## README.md  L2 (new)"), "{shown:?}");
    assert!(has(&shown, "+changed"), "{shown:?}");
    assert!(has(&shown, "Comment: Say why"), "{shown:?}");

    // `C-c C-k` drops it; the comment stays unsent and a new message gathers it again.
    app.send_keys("C-c C-k");
    let state = app.wait_for("the drop", |state| state["buffer"]["name"] != MESSAGE);
    assert_eq!(
        echo(&state),
        "The message was dropped: its comments stay unsent"
    );
    // Killing the message shows the review it was made from again.
    assert_eq!(state["buffer"]["name"], REVIEW);
    app.send_keys("C-c C-c");
    app.wait_for("the message again", |state| {
        state["buffer"]["name"] == MESSAGE
    });

    // The person's words go too; `C-c C-c` pastes it all, not submitted.
    app.send_keys(&format!("M-> RET {}", typing("Thanks")));
    app.send_keys("C-c C-c");
    let read = pastes(&flow, 1);
    assert!(read.contains("## README.md  L2 (new)"), "{read:?}");
    assert!(read.contains("Comment: Say why"), "{read:?}");
    assert!(read.contains("Thanks"), "{read:?}");
    assert!(read.ends_with(END), "{read:?}");
    let said = echo(&app.state());
    assert_eq!(
        said,
        "Pasted into repo: press Enter there. 1 comment marked sent"
    );

    // Sent, it is gathered no more, and its message is gone. The review, an internal buffer,
    // is shown again from its menu, which `C-x b` does not list.
    show_review(&mut app);
    app.send_keys("C-c C-c");
    assert_eq!(echo(&app.state()), "No comment of yours waits to be sent");
    let buffers = app.state()["buffers"].clone();
    assert!(
        !buffers.to_string().contains(MESSAGE),
        "the message's buffer is killed: {buffers}"
    );
    flow.exit_with(0);
}
