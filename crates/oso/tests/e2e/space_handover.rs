//! Replacing a space's orchestrator (ADR 0136, step 5): with no `handoff.md`, the person is
//! asked first; on `y`, the old orchestrator is stopped and kept as `NAME-was-1`, listed as an
//! earlier one, and once it ended its successor starts under the space's name, in the space's
//! directory, with a brief naming whom it takes over from.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test: the brief's words
//! and the earlier name have their own tests.

use std::thread;
use std::time::{Duration, Instant};

use super::session_tools::{FAKE, runs};
use super::space_tools::orchestrated;
use super::spaces::{PrivateRoot, list, space};
use super::worktrees::{DEADLINE, Flow, asks};

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_replaced_orchestrator_is_kept_and_its_successor_takes_over() {
    let spaces = PrivateRoot::new("space-handover");
    let config = format!("\n[spaces]\nroot = \"{}\"\n", spaces.0.display());
    let flow = Flow::with_fake("space-handover", &config, FAKE);
    let (mut app, _bridge, _other) = orchestrated(&flow, &spaces);
    let first = app.wait_for("the orchestrator recorded", |state| {
        space(state, "fix-login").is_some_and(|s| s["orchestrator"].is_string())
    });
    let old = space(&first, "fix-login").unwrap()["orchestrator"].clone();

    // No handoff.md: asked first; `y` replaces it.
    app.send_keys("C-c a S");
    app.send_keys("O");
    app.wait_for("the question", |state| {
        asks(state).is_some_and(|prompt| prompt.starts_with("No handoff.md"))
    });
    app.send_keys("y");

    // Its successor starts under the space's name once the old one ended, told whom it takes
    // over from.
    let directory = std::fs::canonicalize(spaces.0.join("fix-login")).unwrap();
    let deadline = Instant::now() + DEADLINE;
    let successor = loop {
        let found = runs(&flow).into_iter().find(|run| {
            run.cwd == directory && run.args.iter().any(|arg| arg.contains("You take over"))
        });
        if let Some(found) = found {
            break found;
        }
        assert!(Instant::now() < deadline, "no successor started");
        thread::sleep(Duration::from_millis(100));
    };
    assert!(
        successor.args.contains(&"--name=fix-login".to_owned()),
        "{:?}",
        successor.args
    );
    let brief = successor.args.last().unwrap();
    assert!(brief.contains("from session fix-login-was-1"), "{brief}");
    assert!(
        brief.contains("Read handoff.md in this directory first."),
        "{brief}"
    );
    let state = app.wait_for("the successor recorded, the old one earlier", |state| {
        space(state, "fix-login")
            .is_some_and(|s| s["orchestrator"].is_string() && s["orchestrator"] != old)
            && list(state)
                .lines()
                .any(|row| row.contains("fix-login-was-1") && row.contains("earlier"))
    });
    let rows = list(&state);
    assert!(
        rows.lines()
            .any(|row| row.contains("orchestrator") && !row.contains("was-1")),
        "{rows}"
    );
    flow.exit_with(0);
}
