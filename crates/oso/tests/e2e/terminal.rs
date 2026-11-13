//! Terminals (ADR 0120, step 4a): `terminal` starts the configured shell in a PTY, its output
//! is drawn, keys go to it but for oso's own, the PTY takes its window's size, the input method
//! commits into it, a paste asks without bracketed paste, and its end, a kill, an exit and a
//! restart treat a running program as at risk. The agent channel reads and types nothing there,
//! but rows the person let it read (step 4b-1); a password prompt is a secret, a raw read is not.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test: the key
//! routing, the encoder and the policy have their own tests.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::agent::let_in;
use super::harness::{App, Scratch};

/// The configuration every flow starts with: `/bin/sh`, not a login shell, so that no
/// startup file of the person's runs.
const CONFIG: &str = "[terminal]\nshell = \"/bin/sh\"\nlogin = false\nscrollback = 100\n";

fn start(test: &str) -> (Scratch, App) {
    let scratch = Scratch::new(test);
    let config = scratch.write("config.toml", CONFIG.as_bytes());
    let mut app = App::start_with_config(None, scratch.path(), &config);
    app.wait_until_ready();
    (scratch, app)
}

/// The first terminal in the state.
fn terminal(state: &Value) -> &Value {
    &state["terminals"][0]
}

fn rows(state: &Value) -> Vec<String> {
    terminal(state)["rows"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|row| row.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// Starts a terminal and waits for its shell's prompt.
fn open_terminal(app: &mut App) {
    app.run_command("terminal", None).expect("terminal runs");
    app.wait_for("the shell's prompt", |state| {
        terminal(state)["running"] == true && rows(state).iter().any(|row| !row.is_empty())
    });
}

/// Types `line` into the selected window as keys, then `RET`.
fn type_line(app: &mut App, line: &str) {
    let mut keys: Vec<String> = line
        .chars()
        .map(|c| match c {
            ' ' => "SPC".to_owned(),
            c => c.to_string(),
        })
        .collect();
    keys.push("RET".to_owned());
    app.send_keys(&keys.join(" "));
}

/// Whether a row of `state` is `wanted`, or ends in a prompt and then `wanted`: a shell that
/// reads lines (dash) prints what typed-ahead input printed after its next prompt.
fn shows_row(state: &Value, wanted: &str) -> bool {
    let after_prompt = format!("$ {wanted}");
    rows(state)
        .iter()
        .any(|row| row == wanted || row.ends_with(&after_prompt))
}

/// Waits for a row that is `wanted`, as [`shows_row`] reads it.
fn wait_for_row(app: &mut App, wanted: &str) -> Value {
    app.wait_for(wanted, |state| shows_row(state, wanted))
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_terminal_runs_the_shell_and_its_output_is_drawn() {
    let (_scratch, mut app) = start("terminal-start");
    app.visual_aids_off();
    open_terminal(&mut app);
    let state = app.state();
    assert_eq!(state["buffer"]["name"], "*terminal*");
    let modeline = state["modeline"].as_str().unwrap_or_default();
    assert!(modeline.contains("*terminal*"), "{modeline}");
    assert!(modeline.contains("Terminal"), "{modeline}");
    assert_eq!(terminal(&state)["input"], "semi-char");
    type_line(&mut app, "echo hi");
    wait_for_row(&mut app, "hi");
    type_line(&mut app, "echo $TERM_PROGRAM $TERM $COLORTERM");
    wait_for_row(&mut app, "oso xterm-256color truecolor");
    assert!(app.screenshot().foreground().is_some(), "the grid is drawn");
    // `terminal` again switches to it rather than starting another.
    app.send_keys("C-x b");
    type_line(&mut app, "*scratch*");
    app.run_command("terminal", None).unwrap();
    let state = app.wait_for("the terminal again", |state| {
        state["buffer"]["name"] == "*terminal*"
    });
    assert_eq!(state["terminals"].as_array().map(Vec::len), Some(1));
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn oso_keeps_its_keys_and_the_rest_reach_the_program() {
    let (_scratch, mut app) = start("terminal-keys");
    open_terminal(&mut app);
    // C-c C-c sends C-c: the sleep ends, so the echo after it runs at once. An interrupt
    // drops input typed ahead, and a C-c that comes before the shell has started the sleep
    // reaches the shell instead, so both are sent again until the echo runs, as a slow
    // machine may need.
    type_line(&mut app, "sleep 100");
    let mut stopped = false;
    for _ in 0..10 {
        std::thread::sleep(Duration::from_millis(500));
        app.send_keys("C-c C-c");
        type_line(&mut app, "echo stopped");
        let until = Instant::now() + Duration::from_secs(3);
        while Instant::now() < until && !stopped {
            stopped = shows_row(&app.state(), "stopped");
            std::thread::sleep(Duration::from_millis(100));
        }
        if stopped {
            break;
        }
    }
    assert!(
        stopped,
        "no C-c reached the sleep: {:?}",
        rows(&app.state())
    );
    // C-q sends oso's own key as it is.
    type_line(&mut app, "cat -v");
    app.send_keys("C-q C-x RET");
    wait_for_row(&mut app, "^X");
    app.send_keys("C-c C-c");
    // Char mode: C-x goes to the program, C-c C-k comes back.
    app.send_keys("C-c C-k");
    let state = app.wait_for("char mode", |state| terminal(state)["input"] == "char");
    assert!(
        state["modeline"]
            .as_str()
            .is_some_and(|modeline| modeline.contains("char")),
        "{}",
        state["modeline"]
    );
    type_line(&mut app, "cat -v");
    app.send_keys("C-x RET");
    wait_for_row(&mut app, "^X");
    // C-c C-c in char mode sends one C-c, which ends cat.
    app.send_keys("C-c C-c C-c C-k");
    app.wait_for("semi-char mode", |state| {
        terminal(state)["input"] == "semi-char"
    });
    type_line(&mut app, "echo back");
    wait_for_row(&mut app, "back");
    // C-x stays oso's: C-x 2 splits.
    app.send_keys("C-x 2");
    let state = app.wait_for("two windows", |state| {
        state["windows"].as_array().is_some_and(|w| w.len() == 2)
    });
    assert!(state["windows"].is_array());
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn the_pty_takes_the_size_of_the_window_that_selected_it() {
    let (_scratch, mut app) = start("terminal-size");
    open_terminal(&mut app);
    let full = terminal(&app.state())["size"]["rows"].as_u64().unwrap();
    app.send_keys("C-x 2");
    let state = app.wait_for("a smaller terminal", |state| {
        terminal(state)["size"]["rows"]
            .as_u64()
            .is_some_and(|rows| rows < full)
    });
    let size = &terminal(&state)["size"];
    let reported = format!("{} {}", size["rows"], size["columns"]);
    type_line(&mut app, "stty size");
    wait_for_row(&mut app, &reported);
    app.send_keys("C-x 1");
    app.wait_for("the whole frame again", |state| {
        terminal(state)["size"]["rows"].as_u64() == Some(full)
    });
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn the_input_method_commits_into_the_program_at_its_cursor() {
    let (_scratch, mut app) = start("terminal-ime");
    open_terminal(&mut app);
    let state = app.wait_for("the IME on", |state| {
        state["ime"]["allowed"] == true && !state["ime"]["area"].is_null()
    });
    assert_eq!(state["ime"]["wanted"], true);
    app.visual_aids_off();
    type_line(&mut app, "printf '\\033[?25l'");
    // The next prompt first: keys typed ahead are echoed before a shell that reads lines
    // (dash) prints it, and the row would end with the prompt instead.
    app.wait_for("the prompt after printf", |state| {
        let rows = rows(state);
        rows.iter()
            .rposition(|row| row.contains("printf"))
            .is_some_and(|at| {
                rows[at + 1..]
                    .iter()
                    .any(|row| row.trim_end().ends_with('$'))
            })
    });
    app.send_keys("e c h o SPC x");
    let state = app.wait_for("the typed text", |state| {
        rows(state).iter().any(|row| row.ends_with("echo x"))
    });
    // The cursor is hidden, and the candidate window still has its cell.
    let area = &state["ime"]["area"];
    let top = area["y"].as_f64().expect("a cursor area");
    let bottom = top + area["height"].as_f64().expect("its height");
    let (top, bottom) = (top as u32, bottom.ceil() as u32);
    let before = app.screenshot();
    app.try_call("ime", Some(json!({ "preedit": "にほ", "range": [6, 6] })))
        .unwrap();
    app.wait_for("the preedit", |state| state["ime"]["preedit"] == "にほ");
    let composing = app.screenshot();
    // The preedit is drawn on the cursor's row, though the program hides the cursor.
    let changed = composing.differs_in(&before, top..bottom);
    assert!(changed > 0, "nothing drawn on the cursor's row");
    app.try_call("ime", Some(json!({ "commit": "日本" })))
        .unwrap();
    app.send_keys("RET");
    wait_for_row(&mut app, "x日本");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_paste_of_lines_asks_without_bracketed_paste_and_is_cleaned_with_it() {
    let scratch = Scratch::new("terminal-paste");
    let config = scratch.write("config.toml", CONFIG.as_bytes());
    // What the commands print is computed, so that it never reads as the input a shell echoes,
    // which a shell may print on the row of its next prompt (dash does).
    let lines = scratch.write(
        "lines.txt",
        b"echo pasted-$((10+1))\necho pasted-$((10+2))\n",
    );
    let mut app = App::start_with_config(Some(&lines), scratch.path(), &config);
    app.wait_until_ready();
    app.wait_for("the file", |state| state["buffer"]["loading"].is_null());
    app.send_keys("C-x h M-w");
    open_terminal(&mut app);
    app.send_keys("C-y");
    let state = app.wait_for("the question", |state| {
        state["prompts"].as_array().is_some_and(|p| !p.is_empty())
    });
    let label = state["prompts"][0]["prompt"].as_str().unwrap_or_default();
    assert!(label.starts_with("Paste 2 lines into a program"), "{label}");
    app.send_keys("n");
    type_line(&mut app, "echo none");
    wait_for_row(&mut app, "none");
    let printed = |state: &Value, what: &str| rows(state).iter().any(|row| row.ends_with(what));
    assert!(!printed(&app.state(), "pasted-11"));
    app.send_keys("C-y");
    app.wait_for("the question", |state| {
        state["prompts"].as_array().is_some_and(|p| !p.is_empty())
    });
    app.send_keys("y");
    app.wait_for("both lines run", |state| {
        printed(state, "pasted-11") && printed(state, "pasted-12")
    });
    // With bracketed paste on, the text arrives between the brackets.
    type_line(&mut app, "printf '\\033[?2004h'; cat -v");
    app.wait_for("bracketed paste on", |state| {
        terminal(state)["bracketed_paste"] == true
    });
    app.send_keys("C-y");
    app.wait_for("the bracketed text", |state| {
        rows(state)
            .iter()
            .any(|row| row.contains("^[[200~echo pasted-"))
    });
    app.send_keys("C-c C-c");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_running_program_is_asked_about_and_its_end_is_shown() {
    let (_scratch, mut app) = start("terminal-end");
    open_terminal(&mut app);
    // The scrollback keeps about what the configuration says, the library freeing it a page
    // at a time, and the wheel scrolls it.
    type_line(&mut app, "seq 1 5000");
    let state = wait_for_row(&mut app, "5000");
    let history = terminal(&state)["history"].as_u64().unwrap();
    assert!(history > 0 && history < 2000, "{history}");
    app.call("wheel", Some(json!({ "lines": [0.0, 3.0] })));
    app.wait_for("scrolled back", |state| {
        terminal(state)["scrolled"].as_u64().is_some_and(|n| n > 0)
    });
    type_line(&mut app, "echo bottom");
    app.wait_for("back at the bottom", |state| {
        terminal(state)["scrolled"] == 0 && rows(state).iter().any(|row| row == "bottom")
    });
    // Killing it asks while its program runs.
    app.send_keys("C-x k RET");
    let state = app.wait_for("the question", |state| {
        state["prompts"].as_array().is_some_and(|p| !p.is_empty())
    });
    let label = state["prompts"][0]["prompt"].as_str().unwrap_or_default();
    assert!(label.contains("runs a program; kill it anyway?"), "{label}");
    app.send_keys("C-g");
    // So does an exit.
    app.send_keys("C-x C-c");
    let state = app.wait_for("the question", |state| {
        state["prompts"].as_array().is_some_and(|p| !p.is_empty())
    });
    let label = state["prompts"][0]["prompt"].as_str().unwrap_or_default();
    assert!(
        label.starts_with("1 terminal runs a program; exit anyway?"),
        "{label}"
    );
    app.send_keys("n");
    // A running terminal holds a restart.
    let refused = app.exchange("check-handover", None).unwrap();
    assert!(refused.contains("a program runs in it"), "{refused}");
    // Its end stays on screen, and the modeline says how it ended.
    type_line(&mut app, "exit 3");
    let state = app.wait_for("the end", |state| terminal(state)["ended"] == "exited 3");
    assert!(
        state["modeline"]
            .as_str()
            .is_some_and(|modeline| modeline.contains("exited 3")),
        "{}",
        state["modeline"]
    );
    let restart = app.exchange("check-handover", None).unwrap();
    assert!(!restart.contains("a program runs in it"), "{restart}");
    // An ended terminal is killed without asking.
    app.send_keys("C-x k RET");
    let started = Instant::now();
    app.wait_for("no terminal", |state| {
        state["terminals"].as_array().is_some_and(Vec::is_empty)
            || started.elapsed() > Duration::from_secs(10)
    });
    assert_eq!(app.state()["terminals"], json!([]));
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn the_agent_channel_reads_and_types_nothing_in_a_terminal() {
    let (_scratch, mut app) = start("terminal-agent");
    open_terminal(&mut app);
    type_line(&mut app, "echo visible-only-to-the-person");
    wait_for_row(&mut app, "visible-only-to-the-person");
    let mut agent = let_in(&mut app);
    let state = agent.call("state", None).unwrap();
    assert!(
        !state.to_string().contains("visible-only-to-the-person"),
        "{state}"
    );
    let buffer = app.state()["buffer"]["id"].clone();
    let text = agent.call("text", Some(json!({ "buffer": buffer })));
    assert!(text.is_err(), "{text:?}");
    let inserted = agent.call(
        "run",
        Some(json!({ "name": "insert", "arg": "echo typed-by-an-agent", "reach": "change" })),
    );
    assert!(inserted.is_err(), "{inserted:?}");
    for name in ["terminal", "terminal-send-interrupt", "terminal-yank"] {
        let ran = agent.run(name, None, "change");
        assert!(ran.is_err_and(|e| e.contains("needs a person")), "{name}");
    }
    assert!(
        !rows(&app.state())
            .iter()
            .any(|row| row.contains("typed-by-an-agent"))
    );
}

/// Whether `state`'s first terminal is at a password prompt, as its last snapshot said.
fn echo_off(state: &Value) -> bool {
    terminal(state)["echo_off"] == true
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_password_prompt_is_a_secret_and_a_raw_read_is_not() {
    let (_scratch, mut app) = start("terminal-echo-off");
    open_terminal(&mut app);
    type_line(&mut app, "stty -echo; read x; stty echo; echo pw-done");
    let state = app.wait_for("the password prompt", echo_off);
    assert_eq!(state["buffer"]["secret"], true, "{}", state["buffer"]);
    // Dev tooling withholds its rows, and the modeline too, as around any buffer that may
    // hold secrets; the modeline's `secret` mark has unit tests.
    assert!(terminal(&state)["rows"].is_null());
    assert!(state["modeline"].is_null());
    if let Some(wanted) = state["secure_input"]["wanted"].as_bool() {
        assert!(wanted, "secure input is asked for while it is selected");
    }
    type_line(&mut app, "hunter2");
    let state = app.wait_for("the prompt's end", |state| {
        shows_row(state, "pw-done") && !echo_off(state)
    });
    assert_eq!(state["buffer"]["secret"], false);
    assert!(!rows(&state).iter().any(|row| row.contains("hunter2")));

    // A program that reads keys raw turns echo off too, and is no password prompt.
    type_line(
        &mut app,
        "stty raw -echo; echo raw-ready; dd bs=1 count=1 >/dev/null 2>&1; stty sane; echo raw-done",
    );
    let state = wait_for_row(&mut app, "raw-ready");
    assert!(!echo_off(&state), "{}", terminal(&state));
    assert_eq!(state["buffer"]["secret"], false);
    app.send_keys("x");
    wait_for_row(&mut app, "raw-done");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn an_agent_reads_a_terminal_only_once_let_and_never_a_password_prompt() {
    let (_scratch, mut app) = start("terminal-agent-read");
    open_terminal(&mut app);
    type_line(&mut app, "echo shown-to-the-agent");
    wait_for_row(&mut app, "shown-to-the-agent");
    let mut agent = let_in(&mut app);
    let buffer = app.state()["buffer"]["id"].clone();
    let listed = agent.call("state", None).unwrap()["buffers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|listed| listed["id"] == buffer)
        .cloned()
        .unwrap();
    assert_eq!(listed["kind"], "terminal", "{listed}");
    assert_eq!(listed["running"], true, "{listed}");
    assert_eq!(listed["agent_reads"], false, "{listed}");
    let text = || json!({ "buffer": buffer, "start_row": 0, "rows": 100 });
    let refused = agent.call("text", Some(text())).unwrap_err();
    assert!(refused.contains("terminal-allow-agent-read"), "{refused}");
    // Only the person gives the leave.
    let ran = agent.run("terminal-allow-agent-read", None, "change");
    assert!(ran.is_err_and(|e| e.contains("needs a person")));

    app.run_command("terminal-allow-agent-read", None).unwrap();
    let state = app.wait_for("the mark", |state| {
        state["modeline"]
            .as_str()
            .is_some_and(|modeline| modeline.contains("agent reads"))
    });
    assert!(state["modeline"].is_string());
    let read = agent.call("text", Some(text())).unwrap();
    assert!(read.to_string().contains("shown-to-the-agent"), "{read}");

    // At a password prompt the rows are withheld, and the channel waits for the person.
    type_line(&mut app, "stty -echo; read x; stty echo");
    app.wait_for("the password prompt", echo_off);
    let withheld = agent.call("text", Some(text())).unwrap_err();
    assert!(withheld.contains("echo off"), "{withheld}");
    let waits = agent.run("forward-char", None, "look").unwrap_err();
    assert!(waits.contains("may hold secrets"), "{waits}");
    type_line(&mut app, "x");
    app.wait_for("the prompt's end", |state| !echo_off(state));

    app.run_command("terminal-deny-agent-read", None).unwrap();
    let refused = agent.call("text", Some(text())).unwrap_err();
    assert!(refused.contains("terminal-allow-agent-read"), "{refused}");
}
