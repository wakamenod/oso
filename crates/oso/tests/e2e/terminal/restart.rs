//! Terminals over `oso dev`'s restart (ADR 0120, step 11), driven as the supervisor drives
//! two instances: the shell goes on in the new instance with its process, its screen and
//! scrollback; what it printed meanwhile shows once; both screens of a full-screen program
//! come across; an exit refused leaves the terminals with the old instance, reading again;
//! and a password prompt holds the restart.
//!
//! Must not depend on: the app's internals; this drives real instances over their sockets.

use serde_json::{Value, json};

use super::super::harness::{App, Scratch};
use super::{open_terminal, rows, start, terminal, type_line, wait_for_row};

/// A second instance with the flow's configuration.
fn next(scratch: &Scratch) -> App {
    let config = scratch.path().join("config.toml");
    let mut app = App::start_with_config(None, scratch.path(), &config);
    app.wait_until_ready();
    app
}

/// Exports `old`'s state with its terminals for `new`, moves the texts, and imports it.
/// Returns the state.
fn export_and_import(old: &mut App, new: &mut App) -> Value {
    old.call("check-handover", Some(json!({ "terminals": true })));
    let library = new.call("version", None)["terminal_library"].clone();
    let params = json!({ "terminals": { "library": library } });
    let state = old.call("export-state", Some(params));
    let lengths = state["items"].as_array().expect("items").clone();
    for (item, len) in lengths.iter().enumerate() {
        let len = len.as_u64().expect("a length");
        let mut offset = 0;
        while offset < len {
            let at = json!({ "export": state["export"], "item": item, "offset": offset });
            let piece = old.call("export-text", Some(at.clone()));
            let mut params = at;
            params["text"] = piece["text"].clone();
            new.call("import-text", Some(params));
            offset = piece["end"].as_u64().expect("an end");
        }
    }
    new.call("import-state", Some(json!({ "state": state })));
    state
}

/// Hands `old`'s terminals over to `new` as `oso dev` does, and waits for `old` to go.
pub(crate) fn hand_over(old: &mut App, new: &mut App) -> Value {
    let state = export_and_import(old, new);
    let generation = state["generation"].clone();
    let count = state["terminals"].as_array().map_or(0, Vec::len);
    let (handed, masters) = old
        .call_passing(
            "hand-over-terminals",
            Some(json!({ "generation": generation })),
            &[],
        )
        .expect("the masters");
    assert_eq!(handed["terminals"], count);
    assert_eq!(masters.len(), count);
    old.call("exit", Some(json!({ "generation": generation })));
    assert!(old.wait_for_exit().success());
    new.call_passing(
        "import-terminals",
        Some(json!({ "count": count })),
        &masters,
    )
    .expect("the terminals taken");
    state
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_shell_goes_on_in_the_new_instance_with_its_screen_and_what_it_printed_meanwhile() {
    let (scratch, mut old) = start("terminal-restart-shell");
    open_terminal(&mut old);
    type_line(&mut old, "echo before-the-restart");
    wait_for_row(&mut old, "before-the-restart");
    // Prints while the instances change, and after.
    type_line(
        &mut old,
        "for i in 1 2 3 4 5 6 7 8 9 10; do echo tick$i; sleep 0.1; done; echo ticked",
    );
    let mut new = next(&scratch);
    let state = hand_over(&mut old, &mut new);
    assert_eq!(state["terminals"].as_array().map(Vec::len), Some(1));
    let shown = new.wait_for("the ticks", |state| {
        rows(state).iter().any(|row| row.ends_with("ticked"))
    });
    assert_eq!(terminal(&shown)["running"], true);
    let rows = rows(&shown);
    assert!(
        rows.iter().any(|row| row.contains("before-the-restart")),
        "{rows:?}"
    );
    for i in 1..=10 {
        let tick = format!("tick{i}");
        let seen = rows.iter().filter(|row| **row == tick).count();
        assert_eq!(seen, 1, "{tick} once: {rows:?}");
    }
    // The same shell reads on.
    type_line(&mut new, "echo after-the-restart");
    wait_for_row(&mut new, "after-the-restart");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn both_screens_of_a_full_screen_program_come_across() {
    let (scratch, mut old) = start("terminal-restart-screens");
    open_terminal(&mut old);
    type_line(&mut old, "echo on-the-shell-screen");
    wait_for_row(&mut old, "on-the-shell-screen");
    type_line(
        &mut old,
        "printf '\\033[?1049h\\033[Hfull-screen'; read x; printf '\\033[?1049l'",
    );
    old.wait_for("the alternate screen", |state| {
        rows(state).iter().any(|row| row.starts_with("full-screen"))
    });
    let mut new = next(&scratch);
    hand_over(&mut old, &mut new);
    new.wait_for("the alternate screen", |state| {
        rows(state).iter().any(|row| row.starts_with("full-screen"))
    });
    new.send_keys("RET");
    let state = new.wait_for("the shell's screen", |state| {
        rows(state)
            .iter()
            .any(|row| row.contains("on-the-shell-screen"))
    });
    assert!(
        !rows(&state)
            .iter()
            .any(|row| row.starts_with("full-screen"))
    );
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn an_exit_refused_leaves_the_terminals_with_the_old_instance() {
    let (scratch, mut old) = start("terminal-restart-refused");
    open_terminal(&mut old);
    let mut new = next(&scratch);
    let state = export_and_import(&mut old, &mut new);
    // A file edited since the export: the generation is another.
    old.send_keys("C-x b * s c r a t c h * RET");
    old.send_keys("x");
    let refused = old.call_passing(
        "hand-over-terminals",
        Some(json!({ "generation": state["generation"] })),
        &[],
    );
    assert!(refused.is_err(), "{refused:?}");
    new.call("import-terminals", Some(json!({ "abandon": true })));
    new.wait_for("the carried terminal's end", |state| {
        terminal(state)["ended"].is_string()
    });
    // The old one reads its terminal again, soon or after its quiet time.
    old.send_keys("C-x b * t e r m i n a l * RET");
    type_line(&mut old, "echo still-here");
    wait_for_row(&mut old, "still-here");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_password_prompt_holds_the_restart() {
    let (_scratch, mut app) = start("terminal-restart-held");
    open_terminal(&mut app);
    type_line(&mut app, "stty -echo; read secret; stty echo; echo read-it");
    app.wait_for("the password prompt", |state| {
        terminal(state)["echo_off"] == true
    });
    let held = app
        .try_call("check-handover", Some(json!({ "terminals": true })))
        .unwrap_err();
    assert!(held.contains("reads a password"), "{held}");
    type_line(&mut app, "hunter2");
    wait_for_row(&mut app, "read-it");
    app.call("check-handover", Some(json!({ "terminals": true })));
}
