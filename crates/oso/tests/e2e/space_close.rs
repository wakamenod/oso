//! Closing a chosen space (ADR 0136, "A chosen space is closed, as amended"), with `git_agent`'s
//! fake `claude`: one that is not in front, with a session at work, is refused by name, from the
//! prompt and from its row; an idle one is closed by `A` on its row, its session stopped, with
//! the front and the selected pane as they were; and so is the one in front, which leaves the
//! other in front.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test: the prompt's order,
//! its default and what an answer names have their own tests.

use serde_json::Value;

use super::git_agent::{FAKE, WORKING};
use super::harness::App;
use super::sessions_tree::{point_at, point_row};
use super::spaces::{
    PrivateRoot, answer, answer_listed, echo, line_starting, show_readme, space, space_menu,
};
use super::worktrees::Flow;

/// Makes the space called `name`, of `slug`, whose orchestrator is then idle.
fn new_space(app: &mut App, name: &str, slug: &str) {
    show_readme(app);
    app.send_keys("C-c a O");
    answer(app, "Describe the space:", name);
    app.wait_for("the space in front, its orchestrator idle", |state| {
        state["sessions"]["space"] == slug && is(state, slug, "idle")
    });
}

/// Whether session `name` is in `state_is`, as the list shows it.
fn is(state: &Value, name: &str, state_is: &str) -> bool {
    (state["sessions"]["sessions"].as_array()).is_some_and(|rows| {
        rows.iter()
            .any(|row| row["name"] == name && row["state"] == state_is)
    })
}

fn archived(state: &Value, name: &str) -> bool {
    space(state, name).is_some_and(|s| s["archived"] == true)
}

/// Puts point on the row of the space called `name` in the selected `*Spaces*`, folded or not.
fn on_space(app: &mut App, name: &str) {
    let row = |state: &Value| {
        ["▾", "▸"]
            .iter()
            .find_map(|fold| line_starting(state, &format!(" {fold} {name} ")))
    };
    let state = app.wait_for("the space's row", |state| row(state).is_some());
    let row = row(&state).unwrap();
    point_at(app, row);
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_chosen_space_is_closed_and_one_at_work_is_refused_by_name() {
    let root = PrivateRoot::new("space-close");
    let config = format!("\n[spaces]\nroot = \"{}\"\n", root.0.display());
    let flow = Flow::with_fake("space-close", &config, FAKE);
    let (mut app, _) = flow.start();
    new_space(&mut app, "Fix login", "fix-login");
    // Its orchestrator alone runs now, so the screen is its.
    flow.scratch.write("screen", WORKING.as_bytes());
    app.wait_for("at work", |state| is(state, "fix-login", "working"));
    new_space(&mut app, "Second", "second");
    new_space(&mut app, "Third", "third");

    // Chosen while not in front, with a session at work: refused, both named, nothing changed.
    let refused = "Session fix-login of space Fix login is working or needs input; the space \
                   stays open";
    space_menu(&mut app, "c");
    answer_listed(&mut app, "Close space:", "Fix login");
    let state = app.wait_for("the refusal", |state| echo(state) == refused);
    assert!(!archived(&state, "fix-login"));
    assert!(is(&state, "fix-login", "working"));
    assert_eq!(state["sessions"]["space"], "third");

    // From its row too.
    app.send_keys("C-c a s");
    app.wait_for("the list selected", |state| {
        state["buffer"]["name"] == "*Spaces*"
    });
    on_space(&mut app, "Fix login");
    app.send_keys("A");
    let state = app.wait_for("the refusal from the row", |state| echo(state) == refused);
    assert!(!archived(&state, "fix-login"));

    // An idle one that is not in front: closed, its session stopped, the front kept.
    on_space(&mut app, "Second");
    app.send_keys("A");
    let state = app.wait_for("the second closed, its session stopped", |state| {
        archived(state, "second") && is(state, "second", "stopped")
    });
    assert_eq!(state["sessions"]["space"], "third", "the front stays");
    assert_eq!(
        state["buffer"]["name"], "*Spaces*",
        "the pane stays selected"
    );
    assert_eq!(
        echo(&state),
        "Space Second closed; space-reopen opens it again"
    );
    assert!(is(&state, "third", "idle") && is(&state, "fix-login", "working"));
    let state = app.wait_for("its row under the closed ones", |state| {
        line_starting(state, " ▸ Archived spaces ─").is_some()
            && line_starting(state, " ▾ Second ").is_none()
            && line_starting(state, " ▸ Second ").is_none()
    });

    // Point is on the closed ones' heading, not the next space, so a second `A` closes none.
    assert_eq!(
        line_starting(&state, " ▸ Archived spaces ─"),
        Some(point_row(&state))
    );
    app.send_keys("A");
    let state = app.state();
    assert!(!archived(&state, "third") && !archived(&state, "fix-login"));

    // The one in front: the other open one is in front, the pane selected still.
    on_space(&mut app, "Third");
    app.send_keys("A");
    let state = app.wait_for("the third closed", |state| {
        archived(state, "third")
            && is(state, "third", "stopped")
            && state["sessions"]["space"] == "fix-login"
    });
    assert_eq!(state["buffer"]["name"], "*Spaces*");
    assert_eq!(
        echo(&state),
        "Space Third closed; space-reopen opens it again"
    );

    // `A` on a closed one's row closes nothing more.
    let archived_row = line_starting(&state, " ▸ Archived spaces ─");
    point_at(&mut app, archived_row.expect("the closed spaces"));
    app.send_keys("TAB");
    let state = app.wait_for("the closed spaces open", |state| {
        line_starting(state, "   ■ Third").is_some()
    });
    let row = line_starting(&state, "   ■ Third").unwrap();
    point_at(&mut app, row);
    app.send_keys("A");
    app.wait_for("what opens it", |state| echo(state) == "RET opens it again");
    flow.exit_with(0);
}
