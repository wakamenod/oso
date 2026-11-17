//! The phase of the key benchmark with a language server attached (ADR 0138, step 12), against
//! the dev-only fake server, in a trusted project of its own: typing with the server, with 2000
//! diagnostics at one cursor and at 1000, while each change brings a report of 2000, and
//! completion over 5000 items. Every metric is reported only; ADR 0123's way sets thresholds
//! once there are numbers (the author's choice, 2026-11-17).
//!
//! Must not depend on: anything but the harness's own pacing and settling, from the module
//! above, and its temporary directory, which is removed however the phase ends. It spawns
//! nothing: oso starts the server, as it would a person's.
//!
//! - The fake server is the one built beside the app, which `just perf` builds; without one the
//!   phase is skipped with a note, so that an older build still runs.
//! - Unix only: the record of trusted projects names a directory by its device and inode.

use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::{Lcg, TYPING_KEYS, log_step, middle_of_the_file, paced_keys, settle, typed_bytes};
use crate::app::App;
use crate::files::TempDir;
use crate::report::{Report, median, percentile};

/// The lines the file holds in `main`, each with one diagnostic in the reports that have them.
const LINES: usize = 2000;

/// The cursors of the typing with many, each on a line of its own (ADR 0138, Performance).
const CURSORS: usize = 1000;

/// The items of a completion's answer: ADR 0138's most (`COMPLETION_ITEMS_MAX`).
const ITEMS: usize = 5000;

/// What is typed into the popup once it is open: each narrows it, none closes it.
const NARROWING: [&str; 6] = ["i", "t", "e", "m", "1", "2"];

/// How long the server may take to be ready, or a report to be counted.
const READY_WITHIN: Duration = Duration::from_secs(30);

/// How often to look at the state while waiting, and for the popup: short, as `lsp_complete_ms`
/// is measured by looking.
const POLL: Duration = Duration::from_millis(2);

/// The mode line says `lsp` alike while the server starts and once it is ready; what it counts
/// comes only once it is. A server with nothing else to report gives one note, on the closing
/// line, out of view of the typing in the middle.
const READY_MARK: &str = "proj-lsp  lsp I1  ";

/// The fake server's file name beside the app.
const SERVER: &str = "fake-lsp-server";

/// What the server does: the four scripts of the phase.
#[derive(Clone, Copy, PartialEq)]
enum Script {
    /// It answers, and reports only [`READY_MARK`]: plain typing with a server attached.
    Quiet,
    /// A report of [`LINES`] diagnostics once the file opens.
    Diagnosed,
    /// The same, again after each change.
    Reporting,
    /// It completes, with [`ITEMS`] items, and reports only [`READY_MARK`].
    Completing,
}

/// Typing with a server attached, in new processes, as `lsp_type_*`, `lsp_diag_*`,
/// `lsp_report_type_p99_ms`, `lsp_complete_ms` and `lsp_narrow_p99_ms`. `runs` and `count` are
/// at most 2 and 200 (the author's choice, 2026-11-17).
pub(super) fn lsp(
    exe: &Path,
    runs: usize,
    count: usize,
    report: &mut Report,
) -> Result<(), String> {
    if !cfg!(unix) {
        log_step("typing with a language server: skipped, as it runs on Unix only");
        return Ok(());
    }
    let server = exe.with_file_name(SERVER);
    if !server.is_file() {
        log_step("typing with a language server: skipped, as no fake server was built beside oso");
        return Ok(());
    }
    log_step("typing with a language server attached");
    let (runs, count) = (runs.min(2), (count / 2).max(1));
    let dir =
        TempDir::new("lsp").map_err(|e| format!("cannot create a temporary directory: {e}"))?;
    let root = project(&dir)?;
    let mut by_name: Vec<(&str, Vec<f64>)> = [
        "lsp_type_median_ms",
        "lsp_type_p99_ms",
        "lsp_diag_type_p99_ms",
        "lsp_diag_cursors_type_p99_ms",
        "lsp_report_type_p99_ms",
        "lsp_complete_ms",
        "lsp_narrow_p99_ms",
    ]
    .into_iter()
    .map(|name| (name, Vec::new()))
    .collect();
    let mut add = |name: &str, value: f64| {
        if let Some((_, values)) = by_name.iter_mut().find(|(named, _)| *named == name) {
            values.push(value);
        }
    };
    for run in 0..runs {
        let mut rng = Lcg(run as u64 + 1);
        let typed = attached(exe, &server, &dir, &root, Script::Quiet, |app| {
            typing(app, 1, count, &mut rng)
        })?;
        add("lsp_type_median_ms", percentile(&typed, 50.0));
        add("lsp_type_p99_ms", percentile(&typed, 99.0));
        let (one, many) = attached(exe, &server, &dir, &root, Script::Diagnosed, |app| {
            let one = typing(app, 1, count, &mut rng)?;
            many_cursors(app)?;
            Ok((one, typing(app, CURSORS, count, &mut rng)?))
        })?;
        add("lsp_diag_type_p99_ms", percentile(&one, 99.0));
        add("lsp_diag_cursors_type_p99_ms", percentile(&many, 99.0));
        let typed = attached(exe, &server, &dir, &root, Script::Reporting, |app| {
            typing(app, 1, count, &mut rng)
        })?;
        add("lsp_report_type_p99_ms", percentile(&typed, 99.0));
        let (opened, narrowed) = attached(exe, &server, &dir, &root, Script::Completing, |app| {
            completing(app, count, &mut rng)
        })?;
        add("lsp_complete_ms", median(&opened));
        add("lsp_narrow_p99_ms", percentile(&narrowed, 99.0));
    }
    for (name, values) in by_name {
        report.add(name, values);
    }
    Ok(())
}

/// A trusted project of its own, `proj-lsp`, with `src/main.rs` of [`LINES`] lines in `main`;
/// its root, as the system names it, which is what the record holds.
fn project(dir: &TempDir) -> Result<PathBuf, String> {
    let root = dir.file("proj-lsp");
    let made = std::fs::create_dir_all(root.join(".git"))
        .and_then(|()| std::fs::create_dir_all(root.join("src")));
    made.map_err(|e| format!("{}: {e}", root.display()))?;
    let mut text = String::from("fn main() {\n");
    for line in 0..LINES {
        text.push_str(&format!("    let value_{line:04} = {line};\n"));
    }
    text.push_str("}\n");
    let main = root.join("src/main.rs");
    std::fs::write(&main, text).map_err(|e| format!("{}: {e}", main.display()))?;
    std::fs::canonicalize(&root).map_err(|e| format!("{}: {e}", root.display()))
}

/// Runs `measure` in a new process visiting the project's `main.rs` with the fake server on
/// `script`, once the server is ready and, with diagnostics, once they are counted.
fn attached<T>(
    exe: &Path,
    server: &Path,
    dir: &TempDir,
    root: &Path,
    script: Script,
    measure: impl FnOnce(&mut App) -> Result<T, String>,
) -> Result<T, String> {
    let rules = dir.file("fake-lsp.json");
    let wrote = std::fs::write(&rules, rules_for(root, script)?.to_string());
    wrote.map_err(|e| format!("{}: {e}", rules.display()))?;
    let config = dir.file("config.toml");
    let (server, rules) = (utf8(server)?, utf8(&rules)?);
    let toml = format!("[lsp.servers.rust-analyzer]\ncommand = [{server:?}, {rules:?}]\n");
    std::fs::write(&config, toml).map_err(|e| format!("{}: {e}", config.display()))?;
    let record = record_line(root)?;
    let files: [(&str, &[u8]); 1] = [("trusted-projects", record.as_bytes())];
    let main = root.join("src/main.rs");
    let mut app = App::start_with_files(exe, Some(&main), Some(&config), &files)?;
    let measured = ready(&mut app, script).and_then(|()| {
        middle_of_the_file(&mut app)?;
        // RET indents in a Rust file (ADR 0115): off, the text grows by what the keys type.
        let params = json!({ "name": "electric-indent-mode", "arg": -1 });
        app.call("run-command", Some(params))?;
        measure(&mut app)
    });
    app.stop();
    measured
}

/// What the fake server does on `script`, for the file of `root`.
fn rules_for(root: &Path, script: Script) -> Result<Value, String> {
    let mut capabilities = json!({
        "positionEncoding": "utf-8",
        "textDocumentSync": {"openClose": true, "change": 2, "save": true},
    });
    if script == Script::Completing {
        capabilities["completionProvider"] = json!({});
    }
    let mut rules =
        vec![json!({"method": "initialize", "do": [{"reply": {"capabilities": capabilities}}]})];
    // With no version, a report is placed against the text the server was last sent.
    let path = root.join("src/main.rs");
    let uri = oso_lsp::uri::from_path(&path).ok_or("the file has no URI")?;
    let publish = json!({"notify": {"method": "textDocument/publishDiagnostics", "params": {
        "uri": uri,
        "diagnostics": diagnostics(),
    }}});
    let mark = json!({"notify": {"method": "textDocument/publishDiagnostics", "params": {
        "uri": uri,
        "diagnostics": [{
            "range": {"start": {"line": LINES + 1, "character": 0}, "end": {"line": LINES + 1, "character": 1}},
            "severity": 3,
            "message": "ready",
        }],
    }}});
    match script {
        Script::Quiet => rules.push(json!({"method": "textDocument/didOpen", "do": [mark]})),
        Script::Diagnosed => rules.push(json!({"method": "textDocument/didOpen", "do": [publish]})),
        Script::Reporting => {
            rules.push(json!({"method": "textDocument/didOpen", "do": [publish.clone()]}));
            rules.push(json!({"method": "textDocument/didChange", "do": [publish]}));
        }
        Script::Completing => {
            let items: Vec<Value> = (0..ITEMS)
                .map(|item| json!({"label": format!("item{item:04}"), "kind": 6, "detail": "i32"}))
                .collect();
            let reply = json!({"reply": {"isIncomplete": false, "items": items}});
            rules.push(json!({"method": "textDocument/didOpen", "do": [mark]}));
            rules.push(json!({"method": "textDocument/completion", "do": [reply]}));
        }
    }
    rules.push(json!({"method": "shutdown", "do": [{"reply": null}]}));
    Ok(json!({ "rules": rules }))
}

/// One diagnostic on each line of `main`, on its name, of each severity in turn: a quarter of
/// them hints, which are drawn but not counted.
fn diagnostics() -> Vec<Value> {
    (0..LINES)
        .map(|line| {
            let range = json!({
                "start": {"line": line + 1, "character": 8},
                "end": {"line": line + 1, "character": 18},
            });
            json!({
                "range": range,
                "severity": line % 4 + 1,
                "message": format!("unused variable `value_{line:04}`"),
            })
        })
        .collect()
}

/// Waits until the mode line counts what the server reported on open: all of the diagnostics,
/// or the one note that says it is ready.
fn ready(app: &mut App, script: Script) -> Result<(), String> {
    let part = match script {
        Script::Quiet | Script::Completing => READY_MARK.to_owned(),
        Script::Diagnosed | Script::Reporting => {
            let each = LINES / 4;
            format!("proj-lsp  lsp E{each} W{each} I{each}  ")
        }
    };
    let started = Instant::now();
    loop {
        let state = app.call("get-state", None)?;
        let modeline = state["modeline"].as_str().unwrap_or_default();
        if modeline.contains(&part) {
            return settle(app).map(|_| ());
        }
        if started.elapsed() > READY_WITHIN {
            return Err(format!(
                "the mode line should say {part:?} within {READY_WITHIN:?}: {modeline:?}"
            ));
        }
        thread::sleep(POLL * 25);
    }
}

/// Puts a cursor at the start of each of [`CURSORS`] lines of `main` from its first, with
/// `edit-lines`, and fails unless there are that many. From the first, as the typing before has
/// moved the cursor below the middle, fewer than that many lines from the end.
fn many_cursors(app: &mut App) -> Result<(), String> {
    app.send_keys("M-< C-n C-SPC")?;
    let down = json!({ "name": "next-line", "arg": CURSORS - 1 });
    app.call("run-command", Some(down))?;
    app.call("run-command", Some(json!({ "name": "edit-lines" })))?;
    settle(app)?;
    let state = app.call("get-state", None)?;
    let cursors = state["cursor"]["count"].as_u64().unwrap_or_default();
    if cursors != CURSORS as u64 {
        return Err(format!("edit-lines made {cursors} cursors, not {CURSORS}"));
    }
    Ok(())
}

/// Key press to presented frame for `count` keys of typing, after a warm-up for their glyphs,
/// at each of `cursors`. Fails unless each key drew a frame and the text grew by what they type
/// at every cursor, so that it cannot go on measuring keys that have stopped typing.
fn typing(app: &mut App, cursors: usize, count: usize, rng: &mut Lcg) -> Result<Vec<f64>, String> {
    paced_keys(app, &TYPING_KEYS, TYPING_KEYS.len() * 2, rng)?;
    let before = bytes(app)?;
    let samples = paced_keys(app, &TYPING_KEYS, count, rng)?;
    let after = bytes(app)?;
    let typed = typed_bytes(&TYPING_KEYS, count) * cursors as u64;
    if after != before + typed {
        return Err(format!(
            "{count} keys at {cursors} cursors should type {typed} bytes, and took the text from \
             {before} to {after}"
        ));
    }
    if samples.len() != count {
        return Err(format!("{count} typed keys drew {} frames", samples.len()));
    }
    Ok(samples)
}

/// `C-M-i` on a line of its own, until the popup shows its items, then [`NARROWING`] typed
/// into it, `C-g` and the typing killed, until `count` keys narrowed: how long each popup took,
/// as the state shows it, and each narrowing key's press to presented frame.
fn completing(app: &mut App, count: usize, rng: &mut Lcg) -> Result<(Vec<f64>, Vec<f64>), String> {
    app.send_keys("C-e RET")?;
    settle(app)?;
    let rounds = count.div_ceil(NARROWING.len());
    let (mut opened, mut narrowed) = (Vec::with_capacity(rounds), Vec::with_capacity(count));
    // A first round warms the glyphs and the popup's rows up.
    for round in 0..=rounds {
        let sent = Instant::now();
        app.send_keys("C-M-i")?;
        let took = popup_within(app, sent)?;
        settle(app)?;
        let keys = paced_keys(app, &NARROWING, NARROWING.len(), rng)?;
        let state = app.call("get-state", None)?;
        if state["popup"].is_null() {
            return Err(format!(
                "typing {NARROWING:?} closed the popup: {}",
                state["text"]
            ));
        }
        app.send_keys("C-g")?;
        app.send_keys("M-DEL")?;
        settle(app)?;
        if round > 0 {
            opened.push(took);
            narrowed.extend(keys);
        }
    }
    Ok((opened, narrowed))
}

/// Milliseconds from `sent` until the state shows the popup with its rows.
fn popup_within(app: &mut App, sent: Instant) -> Result<f64, String> {
    loop {
        let state = app.call("get-state", None)?;
        let rows = state["popup"]["rows"].as_array().map_or(0, Vec::len);
        if rows > 0 {
            return Ok(sent.elapsed().as_secs_f64() * 1000.0);
        }
        if sent.elapsed() > READY_WITHIN {
            return Err(format!(
                "no popup within {READY_WITHIN:?}: {}",
                state["popup"]
            ));
        }
        thread::sleep(POLL);
    }
}

/// The size of the text.
fn bytes(app: &mut App) -> Result<u64, String> {
    let state = app.call("get-state", None)?;
    let bytes = state["buffer"]["bytes"].as_u64();
    bytes.ok_or("no buffer size in get-state".to_owned())
}

/// The record's line that trusts `root`: its path, device and inode (ADR 0114).
#[cfg(unix)]
fn record_line(root: &Path) -> Result<String, String> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(root).map_err(|e| format!("{}: {e}", root.display()))?;
    Ok(format!(
        "{}\tdev={} ino={}\n",
        utf8(root)?,
        meta.dev(),
        meta.ino()
    ))
}

#[cfg(not(unix))]
fn record_line(_: &Path) -> Result<String, String> {
    Err("the record of trusted projects is written on Unix only".to_owned())
}

fn utf8(path: &Path) -> Result<&str, String> {
    path.to_str()
        .ok_or_else(|| format!("{} is not UTF-8", path.display()))
}
