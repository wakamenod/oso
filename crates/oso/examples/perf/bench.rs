//! The benchmarks: what is measured, on which files and how many times
//! (ADR 0025, ADR 0046). Budgets are in `docs/dev/roadmap.md`.
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
const SETTLE_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_KEYS: Duration = Duration::from_millis(50);
/// The least share of keys that must draw a frame.
const MIN_DRAWN_PERCENT: usize = 95;

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
/// Typing on a screen of text: ASCII, full width and emoji, and a line break,
/// which moves the rows below it.
const TYPING_KEYS: [&str; 12] = [
    "t", "y", "p", "e", "SPC", "日", "本", "語", "SPC", "🍣", "x", "RET",
];
/// Typing in one long line, which a line break would leave behind.
const LINE_TYPING_KEYS: [&str; 8] = ["t", "y", "p", "e", "SPC", "日", "本", "語"];
/// A key that stays unbound: its trip through the UI thread does nothing else.
const UNBOUND_KEY: &str = "<f35>";

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
    let dir =
        TempDir::new("files").map_err(|e| format!("cannot create a temporary directory: {e}"))?;
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
    let spec = BigFile {
        name: "big",
        keys: &SCROLL_KEYS,
        typing: &TYPING_KEYS,
        place: middle_of_the_file,
    };
    big_file(exe, &big, &spec, scale, &mut report)?;
    std::fs::remove_file(&big).map_err(|e| format!("{}: {e}", big.display()))?;

    log_step("the long line");
    let line = dir.file("line.txt");
    let wrote = files::write_long_line(&line, scale.long_line);
    wrote.map_err(|e| format!("{}: {e}", line.display()))?;
    let spec = BigFile {
        name: "line",
        keys: &LINE_KEYS,
        typing: &LINE_TYPING_KEYS,
        place: near_the_end_of_the_first_line,
    };
    big_file(exe, &line, &spec, scale, &mut report)?;
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
/// process started to its first presented frame. A frame skipped while the
/// window is hidden does not count.
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
    let samples = settle(app)?;
    // Every one of these keys moves something. Losing the expensive ones
    // would otherwise look like an improvement.
    if samples.len() * 100 < count * MIN_DRAWN_PERCENT {
        return Err(format!(
            "{} of {count} keys drew a frame; is the window hidden, or a key not bound?",
            samples.len()
        ));
    }
    Ok(samples)
}

/// Waits until no key waits for a frame, and returns the key latencies. A
/// key that never gets a frame, or a dropped sample, would flatter the result,
/// so both are errors. A key withheld around a secret buffer counts as neither
/// (ADR 0060), and the checks on how many keys drew catch that.
fn settle(app: &mut App) -> Result<Vec<f64>, String> {
    let mut samples = Vec::new();
    let started = Instant::now();
    loop {
        let timings = app.timings()?;
        let new = timings["key_latency_us"].as_array().map(Vec::as_slice);
        samples.extend(new.unwrap_or_default().iter().filter_map(us_to_ms));
        if timings["dropped_keys"] != 0 {
            return Err(format!("samples were dropped: {}", timings["dropped_keys"]));
        }
        if timings["keys_waiting"] == 0 {
            return Ok(samples);
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

/// Checks that the far jumps the benchmark relies on really arrive, so that it
/// cannot go on measuring keys that have stopped doing their work.
fn check_jumps(app: &mut App) -> Result<(), String> {
    let cursor_and_size = |app: &mut App| -> Result<(u64, u64), String> {
        let state = app.call("get-state", None)?;
        let offset = state["cursor"]["offset"].as_u64();
        let bytes = state["buffer"]["bytes"].as_u64();
        offset.zip(bytes).ok_or("no cursor in get-state".to_owned())
    };
    app.send_keys("M->")?;
    let (offset, bytes) = cursor_and_size(app)?;
    if offset != bytes || bytes == 0 {
        return Err(format!("M-> went to {offset} of {bytes} bytes"));
    }
    app.send_keys("M-<")?;
    let (offset, _) = cursor_and_size(app)?;
    if offset != 0 {
        return Err(format!("M-< went to {offset}"));
    }
    settle(app).map(|_| ())
}

/// The bytes `count` keys from `keys` type, with a line break of one byte.
fn typed_bytes(keys: &[&str], count: usize) -> u64 {
    let bytes = |key: &&str| {
        if matches!(*key, "SPC" | "RET") {
            1
        } else {
            key.len()
        }
    };
    keys.iter().cycle().take(count).map(bytes).sum::<usize>() as u64
}

/// The size of the text.
fn text_bytes(app: &mut App) -> Result<u64, String> {
    let state = app.call("get-state", None)?;
    let bytes = state["buffer"]["bytes"].as_u64();
    bytes.ok_or("no buffer size in get-state".to_owned())
}

/// Key press to presented frame while typing `keys`, for `count` keys after a
/// warm-up for their glyphs. Fails unless each key drew a frame and the text grew
/// by what they type, so that it cannot go on measuring keys that have stopped
/// typing.
fn typing(app: &mut App, keys: &[&str], count: usize, rng: &mut Lcg) -> Result<Vec<f64>, String> {
    paced_keys(app, keys, keys.len() * 2, rng)?;
    let before = text_bytes(app)?;
    let samples = paced_keys(app, keys, count, rng)?;
    let after = text_bytes(app)?;
    let typed = typed_bytes(keys, count);
    if after != before + typed {
        return Err(format!(
            "{count} keys should type {typed} bytes, and took the text from {before} to {after}"
        ));
    }
    if samples.len() != count {
        return Err(format!("{count} typed keys drew {} frames", samples.len()));
    }
    Ok(samples)
}

/// Key press to presented frame while `DEL` removes what `typing` typed. Only
/// reported. Fails unless each key drew a frame and the text shrank.
fn deleting(app: &mut App, count: usize, rng: &mut Lcg) -> Result<Vec<f64>, String> {
    let before = text_bytes(app)?;
    let samples = paced_keys(app, &["DEL"], count, rng)?;
    let after = text_bytes(app)?;
    if after + count as u64 > before {
        return Err(format!(
            "{count} deleting keys took the text from {before} to {after} bytes"
        ));
    }
    if samples.len() != count {
        return Err(format!(
            "{count} deleting keys drew {} frames",
            samples.len()
        ));
    }
    Ok(samples)
}

/// Key press to presented frame on a screen of mixed text, in new processes.
fn key_latency(exe: &Path, file: &Path, scale: &Scale, report: &mut Report) -> Result<(), String> {
    let (mut p50, mut p99, mut worst) = (Vec::new(), Vec::new(), Vec::new());
    let (mut type_p50, mut type_p99, mut type_worst) = (Vec::new(), Vec::new(), Vec::new());
    let mut del_p99 = Vec::new();
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
        let samples = typing(&mut app, &TYPING_KEYS, scale.keys, &mut rng)?;
        type_p50.push(percentile(&samples, 50.0));
        type_p99.push(percentile(&samples, 99.0));
        type_worst.push(max(&samples));
        let samples = deleting(&mut app, scale.keys, &mut rng)?;
        del_p99.push(percentile(&samples, 99.0));
        app.stop();
    }
    report.add("key_p50_ms", p50);
    report.add("key_p99_ms", p99);
    report.add("key_max_ms", worst);
    report.add("type_p50_ms", type_p50);
    report.add("type_p99_ms", type_p99);
    report.add("type_max_ms", type_worst);
    report.add("del_p99_ms", del_p99);
    Ok(())
}

/// Opens a large file in a window that is already up, as `C-x C-f` will in
/// M2, so that the window's own startup is not part of the numbers.
fn open_file(exe: &Path, file: &Path, scale: &Scale, report: &mut Report) -> Result<(), String> {
    let (mut content, mut screen, mut done, mut launch) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
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

        // The same file from the command line: the window's startup and all.
        let mut app = App::start(exe, Some(file))?;
        let timings = app.wait_until("first screen", FRAME_TIMEOUT, |t| {
            t["load"]["first_screen_after_start_us"].is_u64()
        })?;
        launch.extend(us_to_ms(&timings["load"]["first_screen_after_start_us"]));
        app.stop();
    }
    report.add("open_from_launch_ms", launch);
    report.add("open_first_content_ms", content);
    report.add("open_first_screen_ms", screen);
    report.add("open_full_load_ms", done);
    Ok(())
}

/// One run on a big file: its numbers, each keyed by the end of a metric name.
type RunNumbers = Vec<(&'static str, f64)>;

/// What to do on a big file.
struct BigFile<'a> {
    /// What its metrics start with.
    name: &'a str,
    /// Keys that move around in it.
    keys: &'a [&'a str],
    /// Keys that type in it.
    typing: &'a [&'a str],
    /// Goes to where they type.
    place: fn(&mut App) -> Result<(), String>,
}

/// The line after the middle of a file with many lines, deep in its trees.
fn middle_of_the_file(app: &mut App) -> Result<(), String> {
    let params = json!({ "name": "beginning-of-buffer", "arg": 5 });
    app.call("run-command", Some(params)).map(|_| ())
}

/// Near the end of the first line: for one long line, far along it, and with text
/// after the cursor, since the very end of a buffer skips most of what a key costs.
fn near_the_end_of_the_first_line(app: &mut App) -> Result<(), String> {
    app.send_keys("M-< C-e")?;
    let params = json!({ "name": "backward-char", "arg": 2000 });
    app.call("run-command", Some(params)).map(|_| ())
}

/// Keys and typing on a big file, while it loads and after, in new processes,
/// then edits of the same text in the core structures, which the typing metrics
/// then show from the inside.
fn big_file(
    exe: &Path,
    file: &Path,
    spec: &BigFile<'_>,
    scale: &Scale,
    report: &mut Report,
) -> Result<(), String> {
    let mut metrics: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
    for run in 0..scale.big_runs {
        for (metric, value) in big_file_run(exe, file, spec, scale, run as u64 + 1)? {
            metrics.entry(metric).or_default().push(value);
        }
    }
    for (metric, runs) in metrics {
        report.add(&format!("{}_{metric}", spec.name), runs);
    }
    edit_costs(file, spec.name, scale, report)
}

fn big_file_run(
    exe: &Path,
    file: &Path,
    spec: &BigFile<'_>,
    scale: &Scale,
    seed: u64,
) -> Result<RunNumbers, String> {
    let keys = spec.keys;
    let mut app = App::start(exe, Some(file))?;
    app.wait_until("first frame", FRAME_TIMEOUT, has_frame)?;
    app.wait_until_judged(FRAME_TIMEOUT)?;
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
    if loading.is_empty() {
        log_step("no key was timed while the file loaded: no keys_loading_max_ms this run");
    } else {
        numbers.push(("keys_loading_max_ms", max(&loading)));
    }
    numbers.push(("trip_loading_max_ms", max(&trips)));

    let timings = app.wait_until("loaded file", LOAD_TIMEOUT, |t| {
        t["load"]["done_us"].is_u64()
    })?;
    numbers.extend(us_to_ms(&timings["load"]["done_us"]).map(|load| ("load_ms", load)));
    check_jumps(&mut app)?;

    // Loaded: scrolling.
    let samples = paced_keys(&mut app, keys, scale.big_keys, &mut rng)?;
    numbers.push(("keys_p99_ms", percentile(&samples, 99.0)));
    numbers.push(("keys_max_ms", max(&samples)));

    // Loaded: an unbound key's round trip through the UI thread.
    let mut trips = Vec::with_capacity(scale.big_keys);
    for _ in 0..scale.big_keys {
        let started = Instant::now();
        app.send_keys(UNBOUND_KEY)?;
        trips.push(started.elapsed().as_secs_f64() * 1000.0);
        thread::sleep(KEY_PACE);
    }
    numbers.push(("trip_max_ms", max(&trips)));

    // Loaded: typing. Goes last, since it changes the file's text.
    (spec.place)(&mut app)?;
    settle(&mut app)?;
    let samples = typing(&mut app, spec.typing, scale.big_keys, &mut rng)?;
    numbers.push(("type_p99_ms", percentile(&samples, 99.0)));
    numbers.push(("type_max_ms", max(&samples)));
    let samples = deleting(&mut app, scale.big_keys, &mut rng)?;
    numbers.push(("del_p99_ms", percentile(&samples, 99.0)));
    app.stop();
    Ok(numbers)
}

/// One-character edits and the display map sync, while a snapshot is held, so
/// every edit copies its path. No window is involved.
fn edit_costs(file: &Path, name: &str, scale: &Scale, report: &mut Report) -> Result<(), String> {
    let text = std::fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let mut rope = Rope::from(text.as_str());
    drop(text);
    let mut map = DisplayMap::new(&rope, TabWidth::default());
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
