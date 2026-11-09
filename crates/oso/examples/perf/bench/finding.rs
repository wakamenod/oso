//! The phase of the key benchmark that types into `C-x C-f` over a directory of 100,000 names,
//! which a worker builds and matches (ADR 0096). Only reported, until the author sets a
//! threshold.
//!
//! Must not depend on: anything but the harness's own pacing and settling, from the module
//! above, and its temporary directories, which are removed however the phase ends.

use std::fs::OpenOptions;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::{LOAD_TIMEOUT, Lcg, log_step, paced_keys, settle};
use crate::app::App;
use crate::files::TempDir;
use crate::report::{Report, percentile};

/// How many names the directory holds beside the file the app starts on: with it, the most a
/// prompt lists (ADR 0087).
const NAMES: usize = 99_999;

/// What the names start with, in turn, so that matching folds ASCII, accented and Japanese
/// letters, as a directory of a person's files has them.
const STEMS: [&str; 4] = ["file", "ファイル", "résumé", "Ärger"];

/// What is typed: letters, a regular expression's characters and `DEL` in turn, so that every
/// key leaves a job for the worker, and the answer ends where it began.
const KEYS: [&str; 8] = ["e", ".", "*", "9", "DEL", "DEL", "DEL", "DEL"];

/// How long the worker may take to build the list once the prompt opens.
const BUILT_WITHIN: Duration = Duration::from_secs(60);

/// How often to look whether the list is built.
const POLL: Duration = Duration::from_millis(50);

/// Key press to presented frame in `C-x C-f` over 100,000 names, in new processes, as
/// `file_prompt_type_p99_ms`.
pub(super) fn finding(
    exe: &std::path::Path,
    runs: usize,
    count: usize,
    report: &mut Report,
) -> Result<(), String> {
    log_step("typing into C-x C-f over 100,000 names");
    let dir =
        TempDir::new("finding").map_err(|e| format!("cannot create a temporary directory: {e}"))?;
    let started = Instant::now();
    for index in 0..NAMES {
        let name = format!("{}-{index:05}.txt", STEMS[index % STEMS.len()]);
        let path = dir.file(&name);
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
    }
    log_step(&format!(
        "made {NAMES} files in {:.1} s",
        started.elapsed().as_secs_f64()
    ));
    let start = dir.file("start.txt");
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&start)
        .map_err(|e| format!("{}: {e}", start.display()))?;
    let count = count & !7;
    let mut p99 = Vec::new();
    for run in 0..runs {
        let mut app = App::start(exe, Some(&start))?;
        app.wait_until("loaded file", LOAD_TIMEOUT, |t| {
            t["load"]["done_us"].is_u64()
        })?;
        let mut rng = Lcg(run as u64 + 1);
        app.send_keys("C-x C-f")?;
        settle(&mut app)?;
        built(&mut app)?;
        paced_keys(&mut app, &KEYS, KEYS.len() * 2, &mut rng)?;
        let samples = paced_keys(&mut app, &KEYS, count, &mut rng)?;
        let completion = built(&mut app)?;
        app.send_keys("C-g")?;
        settle(&mut app)?;
        app.stop();
        // Every name, the start file's too, once the answer is back as it began.
        let listed = completion["count"].as_u64();
        if listed != Some(NAMES as u64 + 1) {
            return Err(format!(
                "C-x C-f should end listing {} names; it lists {listed:?}",
                NAMES + 1
            ));
        }
        if samples.len() != count {
            return Err(format!(
                "{count} keys in C-x C-f drew {} frames",
                samples.len()
            ));
        }
        p99.push(percentile(&samples, 99.0));
    }
    report.add("file_prompt_type_p99_ms", p99);
    Ok(())
}

/// Waits until the open prompt's list is built and matched, and returns what dev state says
/// of its completion.
fn built(app: &mut App) -> Result<Value, String> {
    let started = Instant::now();
    loop {
        let state = app.call("get-state", None)?;
        let completion = &state["prompts"][0]["completion"];
        if completion["pending"] == false && completion["total"].as_u64() > Some(0) {
            return Ok(completion.clone());
        }
        if started.elapsed() > BUILT_WITHIN {
            return Err(format!(
                "C-x C-f did not list the directory within {BUILT_WITHIN:?}: {completion}"
            ));
        }
        thread::sleep(POLL);
    }
}
