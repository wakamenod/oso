//! Quitting when idle (ADR 0133, step 5): `C-x C-c` with a job running in a shell names it and
//! offers to wait; `w` leaves the instance running until the job has ended and everything has
//! been idle for a while, and then it exits by itself; `C-g` stops the wait.
//!
//! Must not depend on: the app's internals; this drives a real instance over its socket.

use std::time::{Duration, Instant};

use super::super::harness::App;
use super::{open_terminal, start, type_line};

/// Types `sleep 3` in a new terminal, and asks to exit while it runs.
fn exit_during_a_job(app: &mut App) {
    open_terminal(app);
    type_line(app, "sleep 3");
    // The shell has given the terminal to the job once the question names it.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        app.send_keys("C-x C-c");
        let label = app.state()["prompts"][0]["prompt"]
            .as_str()
            .unwrap_or("")
            .to_owned();
        if label.contains("a job runs in") {
            assert!(label.ends_with("(y, n or w)"), "{label}");
            return;
        }
        assert!(Instant::now() < deadline, "no job named: {label:?}");
        // Not yet: the question, if any, goes, and it asks again.
        app.send_keys("n");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Whether `app` is still running `for_` from now.
fn still_running(app: &mut App, for_: Duration) -> bool {
    let until = Instant::now() + for_;
    while Instant::now() < until {
        if app.exited() {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    true
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn w_exits_once_the_job_has_ended() {
    let (_scratch, mut app) = start("quit-when-idle");
    exit_during_a_job(&mut app);
    app.send_keys("w");
    app.wait_for("the wait said", |state| {
        state["echo"]
            .as_str()
            .is_some_and(|echo| echo.starts_with("Quitting when idle: 1 job under way"))
    });
    assert!(
        still_running(&mut app, Duration::from_secs(1)),
        "it exited while the job ran"
    );
    assert!(app.wait_for_exit().success(), "it exits once idle");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn c_g_stops_the_wait() {
    let (_scratch, mut app) = start("quit-when-idle-cancelled");
    exit_during_a_job(&mut app);
    app.send_keys("w C-g");
    app.wait_for("the cancel said", |state| {
        state["echo"].as_str() == Some("Quit when idle cancelled")
    });
    // The job's 3 s and the hold's 2 s, and more.
    assert!(
        still_running(&mut app, Duration::from_secs(7)),
        "it exited after C-g"
    );
}
