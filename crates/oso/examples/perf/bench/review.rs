//! The review benchmark (ADR 0122): a repository of 57 files with some 5,000 lines changed in
//! its work tree, reviewed with `C-x v r w`: how soon its first screen and its whole text are
//! shown, keys moving between its hunks and files, and `g` after one file changed. Recorded,
//! with no threshold yet.
//!
//! Must not depend on: anything but the harness's own pacing and settling, from the module
//! above, its temporary directories, which are removed however the phase ends, and git, which
//! it starts through the harness's one place for processes (ADR 0046).

use std::fmt::Write as _;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;

use super::{LOAD_TIMEOUT, Lcg, log_step, paced_keys, settle};
use crate::app::App;
use crate::files::TempDir;
use crate::process;
use crate::report::{Report, percentile};

/// How many files the repository holds, each changed.
const FILES: usize = 57;
/// How many lines each file holds as committed.
const LINES: usize = 400;
/// How many hunks each file's change has, and how many lines each replaces: some 5,000 lines
/// removed and added in all.
const HUNKS: usize = 4;
const HUNK_LINES: usize = 11;

/// Keys that move between hunks and files and come back to where they began, the first hunk
/// of the first file, each moving the cursor.
const KEYS: [&str; 7] = ["n", "n", "p", "p", "N", "P", "n"];

/// How long a review may take to be read whole, or to be read again.
const READ_WITHIN: Duration = Duration::from_secs(60);
/// How long the project may take to be found and trusted.
const TRUST_WITHIN: Duration = Duration::from_secs(20);
/// How often to look.
const POLL: Duration = Duration::from_millis(10);

/// Opening, moving in and refreshing a review, in new processes, as
/// `review_open_first_screen_ms`, `review_open_whole_ms`, `review_key_p99_ms` and
/// `review_refresh_ms`.
pub(super) fn review(
    exe: &Path,
    runs: usize,
    count: usize,
    report: &mut Report,
) -> Result<(), String> {
    log_step("reviewing 57 changed files");
    let dir =
        TempDir::new("review").map_err(|e| format!("cannot create a temporary directory: {e}"))?;
    let root = dir.file("repo");
    std::fs::create_dir(&root).map_err(|e| format!("{}: {e}", root.display()))?;
    // An empty directory beside the repository, the only place git looks for hooks.
    let hooks = hooks_of(&root);
    std::fs::create_dir(&hooks).map_err(|e| format!("{}: {e}", hooks.display()))?;
    let mut rng = Lcg(57);
    repository(&root, &mut rng)?;
    let count = (count / KEYS.len()).max(1) * KEYS.len();
    let mut metrics: [Vec<f64>; 4] = Default::default();
    for run in 0..runs {
        let mut app = App::start(exe, Some(&root.join(name(0))))?;
        app.wait_until("loaded file", LOAD_TIMEOUT, |t| {
            t["load"]["done_us"].is_u64()
        })?;
        trust(&mut app)?;
        app.send_keys("C-x v r")?;
        settle(&mut app)?;
        let asked = unix_us();
        app.send_keys("w")?;
        let frames = until_read(&mut app, asked)?;
        let first = frames
            .iter()
            .find(|frame| frame["read"].as_u64() >= Some(1) || frame["done"] == true)
            .ok_or("the review showed no file")?;
        let whole = frames.last().ok_or("the review showed nothing")?;
        metrics[0].push(since(asked, first));
        metrics[1].push(since(asked, whole));

        let mut keys = Lcg(run as u64 + 1);
        app.send_keys("n")?;
        settle(&mut app)?;
        paced_keys(&mut app, &KEYS, KEYS.len() * 2, &mut keys)?;
        let samples = paced_keys(&mut app, &KEYS, count, &mut keys)?;
        if samples.len() != count {
            return Err(format!(
                "{count} keys in the review drew {} frames",
                samples.len()
            ));
        }
        metrics[2].push(percentile(&samples, 99.0));

        change(&root, run)?;
        let asked = unix_us();
        app.send_keys("g")?;
        let frames = until_read(&mut app, asked)?;
        let read = frames.last().ok_or("g showed nothing")?;
        metrics[3].push(since(asked, read));
        app.stop();
    }
    let [first, whole, keys, refresh] = metrics;
    report.add("review_open_first_screen_ms", first);
    report.add("review_open_whole_ms", whole);
    report.add("review_key_p99_ms", keys);
    report.add("review_refresh_ms", refresh);
    Ok(())
}

/// A file's name: in a few directories, as a project's are.
fn name(index: usize) -> String {
    format!("src/part{}/file{index:02}.rs", index % 4)
}

/// One line of a file: something like code, different on each line.
fn line(rng: &mut Lcg, number: usize, out: &mut String) {
    let words = ["let", "value", "match", "self", "return", "items", "count"];
    let _ = write!(out, "    {} x{number} = ", words[rng.below(words.len())]);
    for _ in 0..1 + rng.below(6) {
        let _ = write!(out, "{} ", words[rng.below(words.len())]);
    }
    out.push_str(";\n");
}

/// Makes the repository at `root`: every file committed, then changed in the work tree.
fn repository(root: &Path, rng: &mut Lcg) -> Result<(), String> {
    git(root, &["init", "-q"])?;
    for index in 0..FILES {
        let mut text = String::new();
        for number in 0..LINES {
            line(rng, number, &mut text);
        }
        write(root, index, &text)?;
    }
    git(root, &["add", "-A"])?;
    git(
        root,
        &[
            "-c",
            "user.name=perf",
            "-c",
            "user.email=perf@example.invalid",
            "commit",
            "-q",
            "--no-gpg-sign",
            "-m",
            "base",
        ],
    )?;
    for index in 0..FILES {
        let path = root.join(name(index));
        let old = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut lines: Vec<String> = old.lines().map(|line| format!("{line}\n")).collect();
        // Hunks spread over the file, never touching each other.
        for hunk in 0..HUNKS {
            let start = hunk * (LINES / HUNKS) + rng.below(LINES / HUNKS - HUNK_LINES);
            for (number, kept) in lines.iter_mut().enumerate().skip(start).take(HUNK_LINES) {
                kept.clear();
                line(rng, number + LINES, kept);
            }
        }
        write(root, index, &lines.concat())?;
    }
    Ok(())
}

/// Changes one file again, a line added at its end, which is a new hunk, so that `g` has one
/// file to read.
fn change(root: &Path, run: usize) -> Result<(), String> {
    let path = root.join(name(run % FILES));
    let mut text =
        std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let _ = writeln!(text, "    let again{run} = 1;");
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))
}

fn write(root: &Path, index: usize, text: &str) -> Result<(), String> {
    let path = root.join(name(index));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))
}

/// The empty directory beside the repository at `root` that git looks for hooks in.
fn hooks_of(root: &Path) -> std::path::PathBuf {
    root.with_file_name("hooks")
}

/// Runs git in the repository at `root`, kept from the person's configuration, hooks and git
/// variables, so that the benchmark runs only what it asks.
fn git(root: &Path, args: &[&str]) -> Result<(), String> {
    let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let mut hooks = std::ffi::OsString::from("core.hooksPath=");
    hooks.push(hooks_of(root));
    let output = process::command("git")
        .arg("-c")
        .arg(hooks)
        .args(args)
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", null)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

/// Trusts the project of the file shown, as a person does: `M-x trust-project`, then `y`.
fn trust(app: &mut App) -> Result<(), String> {
    wait_for_state(app, "the project restricted", |state| {
        state["modeline"]
            .as_str()
            .is_some_and(|modeline| modeline.contains("  restricted  "))
    })?;
    let typed: Vec<String> = "trust-project".chars().map(String::from).collect();
    app.send_keys(&format!("M-x {} RET", typed.join(" ")))?;
    app.send_keys("y")?;
    wait_for_state(app, "the project trusted", |state| {
        state["echo"]
            .as_str()
            .is_some_and(|echo| echo.starts_with("Trusted "))
    })?;
    settle(app)?;
    Ok(())
}

fn wait_for_state(app: &mut App, what: &str, done: impl Fn(&Value) -> bool) -> Result<(), String> {
    let started = Instant::now();
    loop {
        let state = app.call("get-state", None)?;
        if done(&state) {
            return Ok(());
        }
        if started.elapsed() > TRUST_WITHIN {
            return Err(format!(
                "no {what} within {TRUST_WITHIN:?}: {}",
                state["echo"]
            ));
        }
        thread::sleep(POLL);
    }
}

/// The frames that showed a review's new text since `asked`, until one shows it read whole.
fn until_read(app: &mut App, asked: u64) -> Result<Vec<Value>, String> {
    let started = Instant::now();
    let mut frames = Vec::new();
    loop {
        let timings = app.timings()?;
        let new = timings["review_frames"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        frames.extend(
            new.into_iter()
                .filter(|frame| frame["unix_us"].as_u64() >= Some(asked)),
        );
        if frames.last().is_some_and(|frame| frame["done"] == true) {
            return Ok(frames);
        }
        if started.elapsed() > READ_WITHIN {
            return Err(format!(
                "the review was not read within {READ_WITHIN:?}: {:?}",
                frames.last()
            ));
        }
        thread::sleep(POLL);
    }
}

fn unix_us() -> u64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    u64::try_from(now.as_micros()).unwrap_or(u64::MAX)
}

/// Milliseconds from `asked` to the frame `frame`.
fn since(asked: u64, frame: &Value) -> f64 {
    let at = frame["unix_us"].as_u64().unwrap_or(asked);
    at.saturating_sub(asked) as f64 / 1000.0
}
