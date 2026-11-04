//! The benchmarks: what is measured, on which files and how many times
//! (ADR 0025, ADR 0044). Budgets are in `docs/dev/roadmap.md`.
//!
//! Must not depend on: `compare`; measuring knows nothing of thresholds.

use std::collections::BTreeMap;
use std::hint::black_box;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use oso_core::display_map::{DisplayMap, TabWidth};
use oso_core::rope::Rope;
use oso_core::sum_tree::Bias;
use serde_json::{Value, json};

use crate::app::App;
use crate::files::{self, TempDir};
use crate::process;
use crate::report::{Gpu, Report, max, median, percentile};

const MB: u64 = 1024 * 1024;
const GB: u64 = 1024 * MB;

/// How long to wait for the first frame, or for a big file to load.
const FRAME_TIMEOUT: Duration = Duration::from_secs(20);
const LOAD_TIMEOUT: Duration = Duration::from_secs(300);
/// How long a key may wait for its frame once the keys have been sent.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(3);

/// About the pace of key repeat. Each key finds the app idle.
const KEY_PACE: Duration = Duration::from_millis(25);
/// Up to this much more, at random, so keys do not lock to the display's
/// refresh and sample only one phase of it.
const KEY_JITTER_MS: usize = 16;

/// Keys that each move the view on a screen of mixed text. They balance out,
/// so the cursor stays away from the ends of the file.
const MIXED_KEYS: [&str; 10] = [
    "C-n", "C-n", "C-f", "C-e", "C-a", "C-p", "C-p", "C-v", "M-v", "C-n",
];
/// Scrolling a big file, with jumps to both of its ends.
const SCROLL_KEYS: [&str; 8] = ["C-v", "C-v", "C-n", "M-v", "C-p", "M->", "M-<", "C-n"];
/// Moving along one long line: the end of the line is the far jump.
const LINE_KEYS: [&str; 6] = ["C-f", "C-f", "C-e", "C-a", "C-f", "C-b"];
/// An unbound key: typing inserts nothing before M2.
const UNBOUND_KEY: &str = "a";

/// How much to measure.
pub struct Scale {
    /// Key samples per run.
    keys: usize,
    /// Runs that restart the app, for the key latency.
    key_runs: usize,
    /// Launches for startup, not counting the first, cold one.
    startups: usize,
    /// Runs on the 100 MB file.
    opens: usize,
    big_file: u64,
    open_file: u64,
    long_line: u64,
    /// Runs on each big file, restarting the app.
    big_runs: usize,
    /// Key samples per run on the big files.
    big_keys: usize,
    /// Edits of the core text structures, per file.
    edits: usize,
}

impl Scale {
    pub const FULL: Self = Self {
        keys: 400,
        key_runs: 5,
        startups: 9,
        opens: 5,
        big_file: GB,
        open_file: 100 * MB,
        long_line: 10 * MB,
        big_runs: 3,
        big_keys: 160,
        edits: 20_000,
    };

    /// For trying the harness out: small files and few samples. The numbers
    /// mean little.
    pub const QUICK: Self = Self {
        keys: 40,
        key_runs: 1,
        startups: 2,
        opens: 1,
        big_file: 20 * MB,
        open_file: 10 * MB,
        long_line: MB,
        big_runs: 1,
        big_keys: 16,
        edits: 500,
    };
}

/// A small deterministic pseudo-random generator, to avoid a dependency.
struct Lcg(u64);

impl Lcg {
    fn below(&mut self, bound: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) as usize) % bound.max(1)
    }
}

/// Runs every benchmark.
pub fn run_all(exe: &Path, scale: &Scale) -> Result<Report, String> {
    let dir = TempDir::new().map_err(|e| format!("cannot create a temporary directory: {e}"))?;
    let mut report = Report::new();
    report.commit = commit();

    log_step("startup");
    report.gpu = startup(exe, scale, &mut report)?;

    log_step("key latency on a screen of mixed text");
    let mixed = dir.file("mixed.txt");
    let wrote = files::write_mixed(&mixed, MB);
    wrote.map_err(|e| format!("{}: {e}", mixed.display()))?;
    key_latency(exe, &mixed, scale, &mut report)?;

    log_step("opening a large file");
    let open = dir.file("open.txt");
    let wrote = files::write_mixed(&open, scale.open_file);
    wrote.map_err(|e| format!("{}: {e}", open.display()))?;
    open_file(exe, &open, scale, &mut report)?;
    std::fs::remove_file(&open).map_err(|e| format!("{}: {e}", open.display()))?;

    log_step("the big file");
    let big = dir.file("big.txt");
    let wrote = files::write_mixed(&big, scale.big_file);
    wrote.map_err(|e| format!("{}: {e}", big.display()))?;
    big_file(
        exe,
        &big,
        &SCROLL_KEYS,
        "big",
        TabWidth::default(),
        scale,
        &mut report,
    )?;
    std::fs::remove_file(&big).map_err(|e| format!("{}: {e}", big.display()))?;

    log_step("the long line");
    let line = dir.file("line.txt");
    let wrote = files::write_long_line(&line, scale.long_line);
    wrote.map_err(|e| format!("{}: {e}", line.display()))?;
    big_file(
        exe,
        &line,
        &LINE_KEYS,
        "line",
        TabWidth::new(4),
        scale,
        &mut report,
    )?;
    Ok(report)
}

#[expect(clippy::disallowed_macros, reason = "the harness reports its progress")]
fn log_step(step: &str) {
    eprintln!("== {step}");
}

/// The commit measured: from CI, or from the checkout.
fn commit() -> Option<String> {
    if let Ok(sha) = std::env::var("GITHUB_SHA") {
        return Some(sha);
    }
    let output = process::command("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn has_frame(timings: &Value) -> bool {
    timings["first_frame_unix_us"].is_u64()
}

fn gpu_of(timings: &Value) -> Option<Gpu> {
    let gpu = timings["gpu"].as_object()?;
    Some(Gpu {
        name: gpu["name"].as_str()?.to_owned(),
        kind: gpu["kind"].as_str()?.to_owned(),
    })
}

fn us_to_ms(value: &Value) -> Option<f64> {
    value.as_u64().map(|us| us as f64 / 1000.0)
}

/// Launches the app with no file. Startup is the time from just before the
/// process started to its first presented frame, a window that is really up.
fn startup(exe: &Path, scale: &Scale, report: &mut Report) -> Result<Option<Gpu>, String> {
    let mut gpu = None;
    let mut from_spawn = Vec::new();
    let mut from_main = Vec::new();
    // The first launch of a new binary compiles shaders. It is reported but
    // not what later launches see.
    for run in 0..=scale.startups {
        let mut app = App::start(exe, None)?;
        // Nothing asks the UI thread anything while it starts up.
        thread::sleep(Duration::from_millis(500));
        let timings = app.wait_until("first frame", FRAME_TIMEOUT, has_frame)?;
        let spawned = timings["first_frame_unix_us"]
            .as_u64()
            .map(|us| app.spawn_to(us));
        if run == 0 {
            report.add("startup_first_launch_ms", spawned.into_iter().collect());
            gpu = gpu_of(&timings);
        } else {
            from_spawn.extend(spawned);
            from_main.extend(us_to_ms(&timings["startup_us"]));
        }
        app.stop();
    }
    report.add("startup_ms", from_spawn);
    report.add("startup_from_main_ms", from_main);
    Ok(gpu)
}

/// Sends `count` keys from `keys`, paced, and returns the latencies of those
/// that drew a frame, in milliseconds.
fn paced_keys(
    app: &mut App,
    keys: &[&str],
    count: usize,
    rng: &mut Lcg,
) -> Result<Vec<f64>, String> {
    for key in keys.iter().cycle().take(count) {
        app.send_keys(key)?;
        let jitter = rng.below(KEY_JITTER_MS) as u64;
        thread::sleep(KEY_PACE + Duration::from_millis(jitter));
    }
    let mut samples = Vec::new();
    let started = Instant::now();
    loop {
        let timings = app.timings()?;
        let new = timings["key_latency_us"].as_array().map(Vec::as_slice);
        samples.extend(new.unwrap_or_default().iter().filter_map(us_to_ms));
        if timings["keys_waiting"] == 0 || started.elapsed() > SETTLE_TIMEOUT {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    // Most of these keys draw; a key at the end of the file may not.
    if samples.len() < count / 2 {
        return Err(format!(
            "{} of {count} keys drew a frame; is the window hidden?",
            samples.len()
        ));
    }
    Ok(samples)
}

/// Key press to presented frame on a screen of mixed text, in new processes.
fn key_latency(exe: &Path, file: &Path, scale: &Scale, report: &mut Report) -> Result<(), String> {
    let (mut p50, mut p99, mut worst) = (Vec::new(), Vec::new(), Vec::new());
    for run in 0..scale.key_runs {
        let mut app = App::start(exe, Some(file))?;
        app.wait_until("loaded file", LOAD_TIMEOUT, |t| {
            t["load"]["done_us"].is_u64()
        })?;
        let mut rng = Lcg(run as u64 + 1);
        // Warm up: glyphs and shaped rows of what the keys bring into view.
        paced_keys(&mut app, &MIXED_KEYS, MIXED_KEYS.len() * 2, &mut rng)?;
        let samples = paced_keys(&mut app, &MIXED_KEYS, scale.keys, &mut rng)?;
        p50.push(percentile(&samples, 50.0));
        p99.push(percentile(&samples, 99.0));
        worst.push(max(&samples));
        app.stop();
    }
    report.add("key_p50_ms", p50);
    report.add("key_p99_ms", p99);
    report.add("key_max_ms", worst);
    Ok(())
}

/// Opens a large file in a window that is already up, as `C-x C-f` will in
/// M2, so that the window's own startup is not part of the numbers.
fn open_file(exe: &Path, file: &Path, scale: &Scale, report: &mut Report) -> Result<(), String> {
    let (mut content, mut screen, mut done) = (Vec::new(), Vec::new(), Vec::new());
    let path = file.to_str().ok_or("the temporary path is not UTF-8")?;
    for _ in 0..scale.opens {
        let mut app = App::start(exe, None)?;
        app.wait_until("first frame", FRAME_TIMEOUT, has_frame)?;
        app.call("open-file", Some(json!({ "path": path })))?;
        let finished =
            |t: &Value| t["load"]["done_us"].is_u64() && t["load"]["first_screen_us"].is_u64();
        let timings = app.wait_until("loaded file", LOAD_TIMEOUT, finished)?;
        let load = &timings["load"];
        content.extend(us_to_ms(&load["first_content_us"]));
        screen.extend(us_to_ms(&load["first_screen_us"]));
        done.extend(us_to_ms(&load["done_us"]));
        app.stop();
    }
    report.add("open_first_content_ms", content);
    report.add("open_first_screen_ms", screen);
    report.add("open_full_load_ms", done);
    Ok(())
}

/// One run on a big file: its numbers, each keyed by the end of a metric name.
type RunNumbers = Vec<(&'static str, f64)>;

/// Keys and "typing" on a big file, while it loads and after, in new
/// processes, then edits of the same text in the core structures. Nothing
/// inserts text in the app before M2, so typing is measured in two halves: an
/// unbound key's trip through the UI thread, and the cost of an edit without
/// a window.
fn big_file(
    exe: &Path,
    file: &Path,
    keys: &[&str],
    name: &str,
    tab: TabWidth,
    scale: &Scale,
    report: &mut Report,
) -> Result<(), String> {
    let mut metrics: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
    for run in 0..scale.big_runs {
        for (metric, value) in big_file_run(exe, file, keys, scale, run as u64 + 1)? {
            metrics.entry(metric).or_default().push(value);
        }
    }
    for (metric, runs) in metrics {
        report.add(&format!("{name}_{metric}"), runs);
    }
    edit_costs(file, name, tab, scale, report)
}

fn big_file_run(
    exe: &Path,
    file: &Path,
    keys: &[&str],
    scale: &Scale,
    seed: u64,
) -> Result<RunNumbers, String> {
    let mut app = App::start(exe, Some(file))?;
    app.wait_until("first frame", FRAME_TIMEOUT, has_frame)?;
    let mut rng = Lcg(seed);
    let mut numbers = Vec::new();

    // While it loads, for as long as that takes, up to the usual sample count.
    let mut loading = Vec::new();
    let mut trips = Vec::new();
    for key in keys.iter().cycle().take(scale.big_keys) {
        let started = Instant::now();
        app.send_keys(key)?;
        trips.push(started.elapsed().as_secs_f64() * 1000.0);
        thread::sleep(KEY_PACE + Duration::from_millis(rng.below(KEY_JITTER_MS) as u64));
        let timings = app.timings()?;
        let new = timings["key_latency_us"].as_array().map(Vec::as_slice);
        loading.extend(new.unwrap_or_default().iter().filter_map(us_to_ms));
        if timings["load"]["done_us"].is_u64() {
            break;
        }
    }
    if !loading.is_empty() {
        numbers.push(("keys_loading_max_ms", max(&loading)));
    }
    numbers.push(("trip_loading_max_ms", max(&trips)));

    let timings = app.wait_until("loaded file", LOAD_TIMEOUT, |t| {
        t["load"]["done_us"].is_u64()
    })?;
    numbers.extend(us_to_ms(&timings["load"]["done_us"]).map(|load| ("load_ms", load)));

    // Loaded: scrolling.
    let samples = paced_keys(&mut app, keys, scale.big_keys, &mut rng)?;
    numbers.push(("keys_p99_ms", percentile(&samples, 99.0)));
    numbers.push(("keys_max_ms", max(&samples)));

    // Loaded: "typing", as a round trip through the UI thread.
    let mut trips = Vec::with_capacity(scale.big_keys);
    for _ in 0..scale.big_keys {
        let started = Instant::now();
        app.send_keys(UNBOUND_KEY)?;
        trips.push(started.elapsed().as_secs_f64() * 1000.0);
        thread::sleep(KEY_PACE);
    }
    numbers.push(("trip_max_ms", max(&trips)));
    app.stop();
    Ok(numbers)
}

/// One-character edits and the display map sync, while a snapshot is held, so
/// every edit copies its path. No window is involved.
fn edit_costs(
    file: &Path,
    name: &str,
    tab: TabWidth,
    scale: &Scale,
    report: &mut Report,
) -> Result<(), String> {
    let text = std::fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let mut rope = Rope::from(text.as_str());
    drop(text);
    let mut map = DisplayMap::new(&rope, tab);
    let snapshot = (rope.clone(), map.snapshot());
    let mut rng = Lcg(3);
    let mut samples = Vec::with_capacity(scale.edits);
    for index in 0..scale.edits {
        let offset = rope.clip_offset(rng.below(rope.len()), Bias::Left);
        let started = Instant::now();
        let edit = if index % 2 == 0 {
            rope.insert(offset, "x")
        } else {
            let end = rope.clip_offset(offset + 1, Bias::Right);
            rope.delete(offset..end)
        };
        map.sync(&rope, &edit);
        samples.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    black_box(&snapshot);
    report.add(&format!("{name}_edit_p50_ms"), vec![median(&samples)]);
    report.add(
        &format!("{name}_edit_p99_ms"),
        vec![percentile(&samples, 99.0)],
    );
    Ok(())
}
