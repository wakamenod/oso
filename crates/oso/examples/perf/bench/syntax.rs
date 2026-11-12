//! The phase of the key benchmark that types into a large Rust file once highlighting has
//! reported, and with `global-font-lock-mode` off, each first in every other run, so that the
//! difference is what
//! colour costs (ADR 0111): per key, mapping the highlights through the edit and posting a
//! job; per frame, a seek in the highlights per row and a step per run. Only reported, until
//! the author sets thresholds.
//!
//! Must not depend on: anything but the harness's own pacing, from the module above, and its
//! temporary directory, which is removed however the phase ends.

use std::io::Write as _;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::json;

use super::{
    KEY_JITTER_MS, KEY_PACE, LOAD_TIMEOUT, Lcg, POLL_KEYS, SETTLE_TIMEOUT, TYPING_KEYS, log_step,
    middle_of_the_file, text_bytes, typed_bytes, us_to_ms,
};
use crate::app::App;
use crate::files::TempDir;
use crate::report::{Report, percentile};

/// The file's size: large, and under the 2 MiB a buffer may have to be highlighted.
const BYTES: usize = 1 << 20;

/// What the file repeats: Rust with every class the queries take.
const CODE: &str = r#"/// A point on the grid, in cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Point {
    pub row: usize,
    pub column: usize,
}

impl Point {
    /// The point `rows` below, kept on the grid.
    pub fn below(self, rows: usize, limit: usize) -> Option<Self> {
        let row = self.row.checked_add(rows)?;
        if row >= limit {
            return None;
        }
        // The column stays where it was.
        Some(Self { row, ..self })
    }
}

fn describe(point: Point) -> String {
    let label = match point.column {
        0 => "start",
        1..=79 => "inside",
        _ => "past the edge",
    };
    format!("{}:{} is {label}\n", point.row, point.column)
}

const ORIGIN: Point = Point { row: 0, column: 0 };
"#;

/// How long the first report may take once the file is loaded.
const REPORTED_WITHIN: Duration = Duration::from_secs(30);

/// What a run of typed keys measured, each in milliseconds: press to presented frame, press to
/// handled, and the frame's preparation on the CPU.
#[derive(Default)]
struct Samples {
    latency: Vec<f64>,
    handled: Vec<f64>,
    prepared: Vec<f64>,
}

/// Typing in the middle of a large Rust file, highlighted and then not, in new processes, as
/// `syntax_*` and `syntax_off_*`.
pub(super) fn highlighted_typing(
    exe: &std::path::Path,
    runs: usize,
    count: usize,
    report: &mut Report,
) -> Result<(), String> {
    log_step("typing into a large highlighted Rust file");
    let dir =
        TempDir::new("syntax").map_err(|e| format!("cannot create a temporary directory: {e}"))?;
    let path = dir.file("big.rs");
    let mut text = String::with_capacity(BYTES + CODE.len());
    while text.len() < BYTES {
        text.push_str(CODE);
    }
    let mut file = std::fs::File::create(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    file.write_all(text.as_bytes())
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let mut on = Vec::new();
    let mut off = Vec::new();
    for run in 0..runs {
        let mut app = App::start(exe, Some(&path))?;
        app.wait_until("loaded file", LOAD_TIMEOUT, |t| {
            t["load"]["done_us"].is_u64()
        })?;
        middle_of_the_file(&mut app)?;
        wait_for_highlights(&mut app)?;
        let mut rng = Lcg(run as u64 + 1);
        // Each phase goes first in every other run, so neither has the warmer caches.
        for on_now in [run % 2 == 0, run % 2 != 0] {
            set_font_lock(&mut app, on_now)?;
            let samples = typing(&mut app, count, &mut rng)?;
            // A highlighting that stopped, or one still on, would measure the other phase.
            if highlighted(&mut app)? != on_now {
                return Err(format!(
                    "highlighting changed while typing, font-lock {on_now}"
                ));
            }
            if on_now { &mut on } else { &mut off }.push(samples);
        }
        app.stop();
    }
    add(report, "syntax", &on);
    add(report, "syntax_off", &off);
    Ok(())
}

/// Each run's median and 99th percentile of what it measured, as `<name>_*_ms`.
fn add(report: &mut Report, name: &str, runs: &[Samples]) {
    let each = |pick: fn(&Samples) -> &Vec<f64>, at: f64| -> Vec<f64> {
        runs.iter().map(|run| percentile(pick(run), at)).collect()
    };
    report.add(&format!("{name}_type_p50_ms"), each(|s| &s.latency, 50.0));
    report.add(&format!("{name}_type_p99_ms"), each(|s| &s.latency, 99.0));
    report.add(&format!("{name}_handle_p99_ms"), each(|s| &s.handled, 99.0));
    report.add(
        &format!("{name}_prepare_p50_ms"),
        each(|s| &s.prepared, 50.0),
    );
    report.add(
        &format!("{name}_prepare_p99_ms"),
        each(|s| &s.prepared, 99.0),
    );
}

/// Whether the shown buffer has highlights.
fn highlighted(app: &mut App) -> Result<bool, String> {
    Ok(app.call("get-state", None)?["buffer"]["highlighted"] == true)
}

/// Turns `global-font-lock-mode` on or off, and when on, waits for the highlights.
fn set_font_lock(app: &mut App, on: bool) -> Result<(), String> {
    let arg = if on { 1 } else { -1 };
    let params = json!({ "name": "global-font-lock-mode", "arg": arg });
    app.call("run-command", Some(params))?;
    if on {
        wait_for_highlights(app)?;
    }
    Ok(())
}

/// Waits until the shown buffer has highlights, so that typing measures them.
fn wait_for_highlights(app: &mut App) -> Result<(), String> {
    let started = Instant::now();
    while !highlighted(app)? {
        if started.elapsed() > REPORTED_WITHIN {
            return Err(format!("no highlights in {REPORTED_WITHIN:?}"));
        }
        thread::sleep(POLL_KEYS);
    }
    Ok(())
}

/// Types `count` keys, paced, after a warm-up, and fails unless each drew a frame and the text
/// grew by what they type.
fn typing(app: &mut App, count: usize, rng: &mut Lcg) -> Result<Samples, String> {
    let keys = TYPING_KEYS.iter().cycle();
    for key in keys.clone().take(TYPING_KEYS.len() * 2) {
        app.send_keys(key)?;
        thread::sleep(KEY_PACE);
    }
    settle(app, &mut Samples::default())?;
    let before = text_bytes(app)?;
    let mut samples = Samples::default();
    for key in keys.take(count) {
        app.send_keys(key)?;
        let jitter = rng.below(KEY_JITTER_MS) as u64;
        thread::sleep(KEY_PACE + Duration::from_millis(jitter));
    }
    settle(app, &mut samples)?;
    let (after, typed) = (text_bytes(app)?, typed_bytes(&TYPING_KEYS, count));
    if after != before + typed {
        return Err(format!(
            "{count} keys took the text from {before} to {after}"
        ));
    }
    if samples.latency.len() != count {
        let drew = samples.latency.len();
        return Err(format!("{count} typed keys drew {drew} frames"));
    }
    Ok(samples)
}

/// Waits until no key waits for a frame, adding what each key measured to `samples`.
fn settle(app: &mut App, samples: &mut Samples) -> Result<(), String> {
    let started = Instant::now();
    loop {
        let timings = app.timings()?;
        let none = Vec::new();
        let latency = timings["key_latency_us"].as_array().unwrap_or(&none);
        samples.latency.extend(latency.iter().filter_map(us_to_ms));
        let spans = timings["key_spans_us"].as_array().unwrap_or(&none);
        for span in spans {
            let ms = |at: usize| span.get(at).and_then(us_to_ms);
            samples.handled.extend(ms(0));
            samples.prepared.extend(ms(3));
        }
        if timings["dropped_keys"] != 0 {
            return Err(format!("samples were dropped: {}", timings["dropped_keys"]));
        }
        if timings["keys_waiting"] == 0 {
            return Ok(());
        }
        if started.elapsed() > SETTLE_TIMEOUT {
            return Err(format!(
                "{} keys got no frame in {SETTLE_TIMEOUT:?}; is the window hidden?",
                timings["keys_waiting"]
            ));
        }
        thread::sleep(POLL_KEYS);
    }
}
