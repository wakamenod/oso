//! The phases of the key benchmark in a split frame (ADR 0112): moving and typing with four
//! windows, two of them on the file typed in and one wrapping a long line, then with sixteen
//! on that file, and `C-x o` among the sixteen. Only reported, until the author sets
//! thresholds.
//!
//! Must not depend on: anything but the harness's own pacing and settling, from the module
//! above, and the files it writes.

use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::{
    KEY_JITTER_MS, KEY_PACE, LOAD_TIMEOUT, Lcg, MB, MIXED_KEYS, Scale, TYPING_KEYS, log_step,
    paced_keys, settle, typing,
};
use crate::app::App;
use crate::files::{self, TempDir};
use crate::report::{Report, percentile};

/// The long line the third of the four windows wraps: long enough that its wrap is a job of
/// its own, short enough that the job ends before the keys begin.
const LINE_BYTES: u64 = MB;

/// The long line's file.
const LINE_NAME: &str = "windows-line.txt";

/// How often to look whether the long line has loaded.
const POLL: Duration = Duration::from_millis(50);

/// Key press to presented frame with four windows and then sixteen, in new processes, as
/// `win4_key_p99_ms`, `win4_type_p99_ms`, `win16_key_p99_ms`, `win16_type_p99_ms` and
/// `win16_other_p99_ms`.
pub(super) fn windows(
    exe: &Path,
    mixed: &Path,
    dir: &TempDir,
    scale: &Scale,
    report: &mut Report,
) -> Result<(), String> {
    log_step("keys with four windows, then sixteen");
    let line = dir.file(LINE_NAME);
    let wrote = files::write_long_line(&line, LINE_BYTES);
    wrote.map_err(|e| format!("{}: {e}", line.display()))?;
    let names = [
        "win4_key_p99_ms",
        "win4_type_p99_ms",
        "win16_key_p99_ms",
        "win16_type_p99_ms",
        "win16_other_p99_ms",
    ];
    let mut runs: [Vec<f64>; 5] = Default::default();
    for run in 0..scale.key_runs {
        let mut app = App::start(exe, Some(mixed))?;
        app.wait_until("loaded file", LOAD_TIMEOUT, |t| {
            t["load"]["done_us"].is_u64()
        })?;
        let mut rng = Lcg(run as u64 + 1);
        let numbers = four_then_sixteen(&mut app, &line, scale.keys, &mut rng);
        app.stop();
        for (runs, number) in runs.iter_mut().zip(numbers?) {
            runs.push(number);
        }
    }
    std::fs::remove_file(&line).map_err(|e| format!("{}: {e}", line.display()))?;
    for (name, runs) in names.into_iter().zip(runs) {
        report.add(name, runs);
    }
    Ok(())
}

/// One run's p99s, in the order [`windows`] names them: `count` keys each with four windows,
/// and half as many with sixteen, so that the phase adds about four minutes to a full run
/// (the author's choice, 2026-11-12).
fn four_then_sixteen(
    app: &mut App,
    line: &Path,
    count: usize,
    rng: &mut Lcg,
) -> Result<[f64; 5], String> {
    let mixed = selected_buffer(&app.call("get-state", None)?)?;
    four_windows(app, line, mixed)?;
    let (key4, type4) = keys_and_typing(app, count, rng)?;
    sixteen_windows(app, mixed)?;
    let half = count / 2;
    let (key16, type16) = keys_and_typing(app, half, rng)?;
    let other16 = other_window(app, half / 2, rng)?;
    Ok([key4, type4, key16, type16, other16])
}

/// Moving, after a warm-up for what the keys bring into view, then typing, in the selected
/// window: the p99 of each.
fn keys_and_typing(app: &mut App, count: usize, rng: &mut Lcg) -> Result<(f64, f64), String> {
    paced_keys(app, &MIXED_KEYS, MIXED_KEYS.len() * 2, rng)?;
    let keys = paced_keys(app, &MIXED_KEYS, count, rng)?;
    let typed = typing(app, &TYPING_KEYS, count, rng)?;
    Ok((percentile(&keys, 99.0), percentile(&typed, 99.0)))
}

/// Splits the frame into the file typed in on the left and, on the right, the same file over
/// `line` wrapped over `*Messages*`, and selects the left one. Fails unless that is what the
/// dev state shows.
fn four_windows(app: &mut App, line: &Path, mixed: u64) -> Result<(), String> {
    let path = line.to_str().ok_or("the temporary path is not UTF-8")?;
    app.send_keys("C-x 3 C-x 2")?;
    app.call("open-file", Some(json!({ "path": path })))?;
    let wrapping = loaded(app, LINE_NAME)?;
    run(app, "toggle-truncate-lines")?;
    app.send_keys("C-x 2 C-h e")?;
    run(app, "windmove-left")?;
    settle(app)?;
    let state = app.call("get-state", None)?;
    // `*Messages*` is internal, so the dev state lists no buffer of it.
    let shown = shown(&state)?;
    if shown[..shown.len().min(3)] != [mixed, mixed, wrapping]
        || shown.len() != 4
        || [mixed, wrapping].contains(&shown[3])
    {
        return Err(format!(
            "four windows should show the file twice, the line and *Messages*: {}",
            state["windows"]
        ));
    }
    let wraps = buffers(&state)?
        .iter()
        .any(|buffer| buffer["id"] == wrapping && buffer["wrapped"] == true);
    if !wraps || selected_buffer(&state)? != mixed {
        return Err("the line should wrap, and the file typed in be selected".into());
    }
    Ok(())
}

/// Lays the frame out as four columns of four windows, all on the file typed in, balanced,
/// with one of them selected. Fails unless the dev state shows sixteen on it.
fn sixteen_windows(app: &mut App, mixed: u64) -> Result<(), String> {
    // Each split selects the new window: the right column, then the bottom of each column.
    app.send_keys("C-x 1 C-x 3 C-x 3 C-x 3 C-x +")?;
    for column in 0..4 {
        app.send_keys("C-x 2 C-x 2 C-x 2")?;
        if column < 3 {
            run(app, "windmove-left")?;
        }
    }
    app.send_keys("C-x +")?;
    settle(app)?;
    let state = app.call("get-state", None)?;
    let shown = shown(&state)?;
    if shown.len() != 16 || shown.iter().any(|buffer| *buffer != mixed) {
        return Err(format!(
            "sixteen windows should show the file: {}",
            state["windows"]
        ));
    }
    Ok(())
}

/// Key press to presented frame for the `o` of `C-x o`, `count` times among the windows,
/// after a warm-up of one round, each key paced as the other phases pace theirs. Its p99. Fails unless each drew a frame and the round ends on
/// the window it began on.
fn other_window(app: &mut App, count: usize, rng: &mut Lcg) -> Result<f64, String> {
    let windows = shown(&app.call("get-state", None)?)?.len();
    let selected = |app: &mut App| -> Result<Value, String> {
        let state = app.call("get-state", None)?;
        let windows = state["windows"]
            .as_array()
            .ok_or("no windows in get-state")?;
        let selected = windows.iter().find(|window| window["selected"] == true);
        selected
            .map(|window| window["id"].clone())
            .ok_or("no window is selected".into())
    };
    let first = selected(app)?;
    let mut samples = Vec::with_capacity(count);
    // Whole rounds, so that it ends where it began, and at least one.
    let count = (count / windows).max(1) * windows;
    for index in 0..windows + count {
        app.send_keys("C-x")?;
        // What the prefix draws is not this key's. Then `o` waits as any key does: sent right
        // after that frame, its own would wait for the display's next refresh to be presented,
        // which measured the pacing and not the key (ADR 0112).
        settle(app)?;
        let pace =
            |rng: &mut Lcg| KEY_PACE + Duration::from_millis(rng.below(KEY_JITTER_MS) as u64);
        thread::sleep(pace(rng));
        app.send_keys("o")?;
        thread::sleep(pace(rng));
        let drawn = settle(app)?;
        if index >= windows {
            samples.extend(drawn);
        }
    }
    if samples.len() != count {
        return Err(format!("{count} C-x o drew {} frames", samples.len()));
    }
    if selected(app)? != first {
        return Err(format!(
            "{count} C-x o among {windows} windows should end where they began"
        ));
    }
    Ok(percentile(&samples, 99.0))
}

/// Runs the command `name`.
fn run(app: &mut App, name: &str) -> Result<(), String> {
    app.call("run-command", Some(json!({ "name": name })))
        .map(|_| ())
}

/// Waits until the buffer named `name`, a file's, has loaded, and returns its id.
fn loaded(app: &mut App, name: &str) -> Result<u64, String> {
    let started = Instant::now();
    loop {
        let state = app.call("get-state", None)?;
        let buffer = buffers(&state)?
            .iter()
            .find(|buffer| buffer["name"] == name);
        if let Some(buffer) = buffer
            && buffer["loading"].is_null()
        {
            return buffer["id"].as_u64().ok_or("a buffer has no id".into());
        }
        if started.elapsed() > LOAD_TIMEOUT {
            return Err(format!("{name} did not load within {LOAD_TIMEOUT:?}"));
        }
        thread::sleep(POLL);
    }
}

fn buffers(state: &Value) -> Result<&Vec<Value>, String> {
    state["buffers"]
        .as_array()
        .ok_or("no buffers in get-state".into())
}

/// The buffer each window shows, in the tree's order.
fn shown(state: &Value) -> Result<Vec<u64>, String> {
    let windows = state["windows"]
        .as_array()
        .ok_or("no windows in get-state")?;
    windows
        .iter()
        .map(|window| {
            window["buffer"]
                .as_u64()
                .ok_or("a window shows no buffer".to_owned())
        })
        .collect()
}

/// The buffer the selected window shows.
fn selected_buffer(state: &Value) -> Result<u64, String> {
    let windows = state["windows"]
        .as_array()
        .ok_or("no windows in get-state")?;
    let selected = windows.iter().find(|window| window["selected"] == true);
    selected
        .and_then(|window| window["buffer"].as_u64())
        .ok_or("no window is selected".into())
}
