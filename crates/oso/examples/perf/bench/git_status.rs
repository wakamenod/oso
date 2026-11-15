//! The git status benchmark (ADR 0127): a repository of 10,000 files, each changed in its work
//! tree, its status shown with `C-x g`, then read again with `g` while keys are typed in another
//! window. Report-only: none of its metrics has a threshold yet.
//!
//! Must not depend on: anything but the harness's own pacing and settling, from the module
//! above, its temporary directories, which are removed however the phase ends, and the review
//! benchmark's git and trust, which start git through the harness's one place for processes
//! (ADR 0046).

use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use super::review::{git, hooks_of, trust};
use super::{LOAD_TIMEOUT, Lcg, log_step, paced_keys, settle};
use crate::app::App;
use crate::files::TempDir;
use crate::report::{Report, percentile};

/// How many files the repository holds, each changed.
const FILES: usize = 10_000;

/// What is typed in the file's window while the status is read again: letters, then as many
/// `DEL`s, so that the file ends as it began.
const KEYS: [&str; 8] = ["t", "y", "p", "e", "DEL", "DEL", "DEL", "DEL"];

/// How long a status may take to show every file.
const SHOWN_WITHIN: Duration = Duration::from_secs(60);
/// How often to look whether it has.
const POLL: Duration = Duration::from_millis(5);

/// Shows the status with `C-x g`, as `git_status_open_ms` (until every changed file is listed),
/// then reads it again with `g` and types in the other window meanwhile, as
/// `git_status_refresh_type_p99_ms`.
pub(super) fn git_status(
    exe: &Path,
    runs: usize,
    count: usize,
    report: &mut Report,
) -> Result<(), String> {
    log_step("the git status of 10,000 changed files");
    let dir = TempDir::new("git-status")
        .map_err(|e| format!("cannot create a temporary directory: {e}"))?;
    let root = dir.file("repo");
    std::fs::create_dir(&root).map_err(|e| format!("{}: {e}", root.display()))?;
    let hooks = hooks_of(&root);
    std::fs::create_dir(&hooks).map_err(|e| format!("{}: {e}", hooks.display()))?;
    repository(&root)?;
    let count = count & !7;
    let (mut open, mut typing) = (Vec::new(), Vec::new());
    for run in 0..runs {
        let mut app = App::start(exe, Some(&root.join("start.txt")))?;
        app.wait_until("loaded file", LOAD_TIMEOUT, |t| {
            t["load"]["done_us"].is_u64()
        })?;
        trust(&mut app)?;
        app.send_keys("C-x 3")?;
        settle(&mut app)?;
        let asked = Instant::now();
        app.send_keys("C-x g")?;
        open.push(until_listed(&mut app, asked)?);
        app.send_keys("g")?;
        app.send_keys("C-x o")?;
        let mut rng = Lcg(run as u64 + 1);
        let samples = paced_keys(&mut app, &KEYS, count, &mut rng)?;
        typing.push(percentile(&samples, 99.0));
        app.stop();
    }
    report.add("git_status_open_ms", open);
    report.add("git_status_refresh_type_p99_ms", typing);
    Ok(())
}

/// Makes the repository at `root`: every file and `start.txt` committed, then each file but
/// `start.txt` changed in the work tree.
fn repository(root: &Path) -> Result<(), String> {
    git(root, &["init", "-q"])?;
    let path = |index: usize| root.join(format!("d{}/file{index:05}.txt", index % 100));
    for index in 0..FILES {
        let file = path(index);
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::write(&file, format!("line {index}\n"))
            .map_err(|e| format!("{}: {e}", file.display()))?;
    }
    let start = root.join("start.txt");
    std::fs::write(&start, "start\n").map_err(|e| format!("{}: {e}", start.display()))?;
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
        let file = path(index);
        std::fs::write(&file, format!("line {index}\nchanged\n"))
            .map_err(|e| format!("{}: {e}", file.display()))?;
    }
    Ok(())
}

/// Milliseconds from `asked` until the selected window shows a status that lists every file.
fn until_listed(app: &mut App, asked: Instant) -> Result<f64, String> {
    loop {
        let state = app.call("get-state", None)?;
        let buffer = &state["buffer"];
        let status = buffer["name"]
            .as_str()
            .is_some_and(|name| name.starts_with("*git: "));
        let lines = buffer["lines"].as_u64().unwrap_or(0);
        if status && lines > FILES as u64 {
            return Ok(asked.elapsed().as_secs_f64() * 1000.0);
        }
        if asked.elapsed() > SHOWN_WITHIN {
            return Err(format!(
                "the status did not list every file within {SHOWN_WITHIN:?}: {buffer}"
            ));
        }
        thread::sleep(POLL);
    }
}
