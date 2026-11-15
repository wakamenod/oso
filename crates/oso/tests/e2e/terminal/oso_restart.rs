//! Terminals over `oso-restart` (ADR 0133, step 2): the instance going starts its own binary
//! with one end of a socket pair, and the shell goes on in the new instance, the same process
//! with its screen; a password prompt holds the restart; a new instance that ends before it
//! speaks, or an exit refused once the new one holds everything, leaves the terminals with the
//! instance going, reading again, and the new one closes.
//!
//! Must not depend on: the app's internals; this drives real instances over their sockets.

use std::time::{Duration, Instant};

use super::super::harness::{App, Scratch, sockets};
use super::{CONFIG, open_terminal, rows, shows_row, start, type_line, wait_for_row};

/// Starts an instance whose restart stops short at `point` (a dev build's hook).
fn start_failing_at(test: &str, point: &str) -> (Scratch, App) {
    let scratch = Scratch::new(test);
    let config = scratch.write("config.toml", CONFIG.as_bytes());
    let set = [("OSO_DEV_RESTART_FAIL", std::ffi::OsStr::new(point))];
    let mut app = App::start_with_config_and_env(scratch.path(), &config, &set);
    app.wait_until_ready();
    (scratch, app)
}

/// The shell's process id, as `echo pid=$$` printed it.
fn shell_pid(app: &mut App) -> String {
    type_line(app, "echo pid=$$");
    let state = app.wait_for("the shell's pid", |state| {
        rows(state).iter().any(|row| row.starts_with("pid="))
    });
    rows(&state)
        .into_iter()
        .find_map(|row| row.strip_prefix("pid=").map(str::to_owned))
        .expect("a pid")
}

/// Runs `oso-restart` in `app`, and returns the sockets there were before.
fn restart(app: &mut App) -> Vec<u32> {
    let before = sockets();
    app.run_command("oso-restart", None)
        .expect("oso-restart runs");
    before
}

/// Waits until `app`'s echo area says `wanted`.
fn wait_for_echo(app: &mut App, wanted: &str) {
    app.wait_for(wanted, |state| {
        state["echo"]
            .as_str()
            .is_some_and(|echo| echo.contains(wanted))
    });
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_shell_goes_on_in_the_instance_oso_restart_starts() {
    let (_scratch, mut old) = start("oso-restart-shell");
    open_terminal(&mut old);
    type_line(&mut old, "KEPT=kept-across");
    let pid = shell_pid(&mut old);
    let before = restart(&mut old);
    let mut new = App::started_by_a_restart(&before);
    assert!(old.wait_for_exit().success(), "the instance going exits");
    new.wait_until_ready();
    new.wait_for("the screen from before", |state| {
        shows_row(state, &format!("pid={pid}"))
    });
    // The same shell, with what it was told before, reads on.
    type_line(&mut new, "echo $KEPT-$$");
    wait_for_row(&mut new, &format!("kept-across-{pid}"));
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_password_prompt_holds_oso_restart() {
    let (_scratch, mut old) = start("oso-restart-password");
    open_terminal(&mut old);
    type_line(&mut old, "stty -echo; read x; stty echo; echo pw-done");
    old.wait_for("the password prompt", |state| {
        super::terminal(state)["echo_off"] == true
    });
    let before = restart(&mut old);
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(sockets().len(), before.len(), "no instance starts");
    type_line(&mut old, "secret");
    wait_for_row(&mut old, "pw-done");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_new_instance_that_ends_before_it_speaks_leaves_the_terminal_here() {
    let (_scratch, mut old) = start_failing_at("oso-restart-hello", "hello");
    open_terminal(&mut old);
    let pid = shell_pid(&mut old);
    restart(&mut old);
    wait_for_echo(&mut old, "everything stays here");
    type_line(&mut old, "echo still-$$");
    wait_for_row(&mut old, &format!("still-{pid}"));
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn an_exit_refused_tells_the_new_instance_to_stay_out_and_it_closes() {
    let (_scratch, mut old) = start_failing_at("oso-restart-exit", "exit");
    open_terminal(&mut old);
    let pid = shell_pid(&mut old);
    let before = restart(&mut old);
    let mut new = App::started_by_a_restart(&before);
    wait_for_echo(&mut old, "everything stays here");
    let since = Instant::now();
    while !new.exited() {
        assert!(
            since.elapsed() < Duration::from_secs(20),
            "the new instance did not close"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    // The terminal reads again here, the same shell.
    type_line(&mut old, "echo here-$$");
    wait_for_row(&mut old, &format!("here-{pid}"));
}
